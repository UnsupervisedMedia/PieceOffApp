//! Audio playback. FFmpeg decodes the clip's audio to PCM and this module
//! plays it through the system output device, choosing which samples to feed
//! so that what is heard follows the shared room clock.

use super::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64};

/// Called each time audio (re)starts from a position. `None` on the app means
/// real playback; tests inject a recorder so resync can be checked silently
/// and without an audio device.
pub(crate) type AudioLauncher =
    Arc<dyn Fn(&Path, AudioCommand) -> Option<std::process::Child> + Send + Sync>;

#[derive(Clone, Copy)]
pub(crate) struct AudioCommand {
    pub(crate) playing: bool,
    pub(crate) position_ms: u64,
    pub(crate) rate: f32,
    pub(crate) loop_start_ms: u64,
    pub(crate) loop_end_ms: u64,
    pub(crate) volume: u8,
    pub(crate) muted: bool,
    /// The timeline is unchanged; only re-read the clock and levels.
    pub(crate) resync: bool,
    /// When `position_ms` was read from the room clock. The worker may be
    /// busy when the command arrives, so it must not assume "now".
    pub(crate) issued_at: Instant,
}

/// Audio is off for `cargo test` always, and for any run with
/// `PIECEOFF_NO_AUDIO` set (visual/automated runs).
pub(crate) fn audio_disabled() -> bool {
    cfg!(test) || std::env::var_os("PIECEOFF_NO_AUDIO").is_some()
}

pub(crate) fn atempo_filter(rate: f32) -> String {
    let mut remaining = rate.clamp(0.25, 2.0);
    let mut stages = Vec::new();
    while remaining < 0.5 {
        stages.push("atempo=0.5".to_owned());
        remaining /= 0.5;
    }
    while remaining > 2.0 {
        stages.push("atempo=2.0".to_owned());
        remaining /= 2.0;
    }
    stages.push(format!("atempo={remaining:.3}"));
    stages.join(",")
}

/// Where in the loop a command's audio starts, in milliseconds.
fn audio_start_position(command: &AudioCommand, position_ms: u64) -> u64 {
    let span = command.loop_end_ms.saturating_sub(command.loop_start_ms);
    if span == 0 {
        position_ms
    } else if position_ms >= command.loop_end_ms {
        command.loop_start_ms
    } else {
        position_ms.max(command.loop_start_ms)
    }
}

/// FFmpeg arguments that decode one pass of the loop from `position_ms` as
/// raw PCM: the seek (placed before the input) and everything after it. The
/// pass ends exactly at the loop end, so passes join without a gap.
pub(crate) fn audio_decode_args(
    command: &AudioCommand,
    position_ms: u64,
    sample_rate: u32,
    channels: u16,
) -> (String, Vec<String>) {
    let position = audio_start_position(command, position_ms);
    let rate = f64::from(command.rate.clamp(0.25, 2.0));
    let mut args = vec!["-vn".into(), "-af".into(), atempo_filter(command.rate)];
    if command.loop_end_ms > command.loop_start_ms {
        // `-t` counts output time, which the tempo filter has stretched.
        let remaining_ms = command.loop_end_ms.saturating_sub(position) as f64;
        args.extend(["-t".into(), format!("{:.3}", remaining_ms / rate / 1000.0)]);
    }
    args.extend([
        "-f".into(),
        "f32le".into(),
        "-ar".into(),
        sample_rate.to_string(),
        "-ac".into(),
        channels.to_string(),
        "pipe:1".into(),
    ]);
    (format!("{:.3}", position as f64 / 1000.0), args)
}

struct AudioSource {
    child: std::process::Child,
    reader: BufReader<std::process::ChildStdout>,
}

fn start_audio_source(
    path: &Path,
    command: &AudioCommand,
    position_ms: u64,
    sample_rate: u32,
    channels: u16,
) -> Option<AudioSource> {
    let (seek, args) = audio_decode_args(command, position_ms, sample_rate, channels);
    let mut child = Command::new(media_tools::ffmpeg())
        .args(["-v", "error", "-ss", &seek, "-i"])
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    Some(AudioSource {
        child,
        reader: BufReader::with_capacity(64 * 1024, stdout),
    })
}

fn stop_audio_source(source: &mut Option<AudioSource>) {
    if let Some(mut source) = source.take() {
        let _ = source.child.kill();
        let _ = source.child.wait();
    }
}

/// Read up to `samples.len()` PCM samples; fewer means the pass has ended.
fn read_samples(source: &mut AudioSource, bytes: &mut Vec<u8>, samples: &mut [f32]) -> usize {
    bytes.resize(samples.len() * 4, 0);
    let mut filled = 0;
    while filled < bytes.len() {
        match source.reader.read(&mut bytes[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    let complete = filled / 4;
    for (sample, raw) in samples.iter_mut().zip(bytes.chunks_exact(4)).take(complete) {
        *sample = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    }
    complete
}

/// State shared with the device callback.
struct OutputShared {
    ring: Mutex<VecDeque<f32>>,
    gain_bits: AtomicU32,
    /// Delay between the callback and the samples reaching the speaker.
    latency_us: AtomicU64,
}

struct AudioOutput {
    stream: cpal::Stream,
    shared: Arc<OutputShared>,
    sample_rate: u32,
    channels: u16,
}

fn fill_output<T>(data: &mut [T], info: &cpal::OutputCallbackInfo, shared: &OutputShared)
where
    T: cpal::Sample + cpal::FromSample<f32>,
{
    let timestamp = info.timestamp();
    shared.latency_us.store(
        timestamp
            .playback
            .duration_since(timestamp.callback)
            .as_micros() as u64,
        Ordering::Relaxed,
    );
    let gain = f32::from_bits(shared.gain_bits.load(Ordering::Relaxed));
    // Never block the audio thread: a contended buffer plays as silence.
    let mut ring = shared.ring.try_lock().ok();
    for sample in data.iter_mut() {
        let value = ring.as_mut().and_then(|ring| ring.pop_front());
        *sample = T::from_sample(value.unwrap_or(0.0) * gain);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: Arc<OutputShared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device.build_output_stream(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| fill_output(data, info, &shared),
        // Underruns play as silence and the feeder re-times itself.
        |_error| {},
        None,
    )
}

fn open_audio_output() -> Result<AudioOutput, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no audio output device was found")?;
    let supported = device
        .default_output_config()
        .map_err(|error| error.to_string())?;
    let config = supported.config();
    let shared = Arc::new(OutputShared {
        ring: Mutex::new(VecDeque::new()),
        gain_bits: AtomicU32::new(0.0_f32.to_bits()),
        latency_us: AtomicU64::new(0),
    });
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, shared.clone()),
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, shared.clone()),
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, shared.clone()),
        cpal::SampleFormat::I32 => build_stream::<i32>(&device, config, shared.clone()),
        cpal::SampleFormat::U8 => build_stream::<u8>(&device, config, shared.clone()),
        format => return Err(format!("unsupported output sample format {format}")),
    }
    .map_err(|error| error.to_string())?;
    Ok(AudioOutput {
        stream,
        shared,
        sample_rate: config.sample_rate,
        channels: config.channels,
    })
}

/// Slider volume (0–100) to linear gain; squared so the low end is usable.
pub(crate) fn volume_gain(volume: u8, muted: bool) -> f32 {
    if muted {
        0.0
    } else {
        (f32::from(volume.min(100)) / 100.0).powi(2)
    }
}

/// Audio queued ahead of the speaker. Small enough that volume and seeks feel
/// immediate, large enough to ride out scheduling hiccups.
const TARGET_BUFFER_MS: f64 = 120.0;
const FEED_CHUNK_MS: f64 = 20.0;
/// Heard audio may wander this far from the room clock before it is corrected.
pub(crate) const AUDIO_SYNC_TOLERANCE_MS: f64 = 60.0;
/// Further out than this, seek the decoder instead of skipping through.
const AUDIO_RESEEK_MS: f64 = 3000.0;
const AUDIO_PRESTART_MS: f64 = 2000.0;

/// What the feeder should do about the gap between heard audio and the clock.
#[derive(Debug, PartialEq)]
pub(crate) enum SyncAction {
    /// Close enough: keep feeding.
    Feed,
    /// Audio is ahead: feed nothing for a moment.
    Wait,
    /// Audio is behind: drop this much media time, in milliseconds.
    Skip(f64),
    /// Too far out to correct smoothly: restart at the clock.
    Reseek,
}

/// `lead_ms` is how far heard audio is ahead of the room clock.
pub(crate) fn sync_action(lead_ms: f64) -> SyncAction {
    if lead_ms.abs() > AUDIO_RESEEK_MS {
        SyncAction::Reseek
    } else if lead_ms > AUDIO_SYNC_TOLERANCE_MS {
        SyncAction::Wait
    } else if lead_ms < -AUDIO_SYNC_TOLERANCE_MS {
        SyncAction::Skip(-lead_ms)
    } else {
        SyncAction::Feed
    }
}

/// One-time alignment after a (re)start, when the exact start delay is known.
pub(crate) fn initial_alignment(lead_ms: f64) -> SyncAction {
    if lead_ms.abs() > AUDIO_RESEEK_MS {
        SyncAction::Reseek
    } else if lead_ms > 5.0 {
        SyncAction::Wait
    } else if lead_ms < -5.0 {
        SyncAction::Skip(-lead_ms)
    } else {
        SyncAction::Feed
    }
}

pub(crate) fn run_audio_player(
    path: PathBuf,
    commands: mpsc::Receiver<AudioCommand>,
    errors: mpsc::Sender<String>,
    launcher: Option<AudioLauncher>,
    offset_ms: Arc<AtomicI32>,
) {
    let mut current = AudioCommand {
        playing: false,
        position_ms: 0,
        rate: 1.0,
        loop_start_ms: 0,
        loop_end_ms: 0,
        volume: 80,
        muted: false,
        resync: false,
        issued_at: Instant::now(),
    };
    let mut clock = MediaClock::new(Instant::now(), 0, 1.0, 0, 0);
    let mut output: Option<AudioOutput> = None;
    let mut output_failed = false;
    let mut source: Option<AudioSource> = None;
    // The next pass of the loop, decoded ahead so the wrap has no gap.
    let mut next_pass: Option<AudioSource> = None;
    // Media time of the next sample `source` will produce.
    let mut cursor_ms = 0.0_f64;
    // Smoothed lead of heard audio over the clock; raw until first settled.
    let mut smoothed_lead_ms = 0.0_f64;
    let mut settled = false;
    // Frames the current pass has produced so far.
    let mut pass_frames = 0_usize;
    let mut bytes = Vec::new();
    let mut samples: Vec<f32> = Vec::new();
    // `PIECEOFF_AUDIO_DEBUG=1` prints how far heard audio is from the clock.
    let debug_sync = std::env::var_os("PIECEOFF_AUDIO_DEBUG").is_some();
    let mut last_report = Instant::now();
    loop {
        let feeding = current.playing && source.is_some();
        let received = if feeding {
            commands.try_recv().map_err(|error| match error {
                mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
                mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
            })
        } else {
            commands.recv_timeout(Duration::from_millis(50))
        };
        match received {
            Ok(command) => {
                let same_timeline = command.playing == current.playing
                    && command.rate == current.rate
                    && command.loop_start_ms == current.loop_start_ms
                    && command.loop_end_ms == current.loop_end_ms;
                clock = MediaClock::new(
                    command.issued_at,
                    command.position_ms,
                    command.rate,
                    command.loop_start_ms,
                    command.loop_end_ms,
                );
                let restart = !(command.resync && same_timeline);
                current = command;
                if let Some(output) = &output {
                    output.shared.gain_bits.store(
                        volume_gain(current.volume, current.muted).to_bits(),
                        Ordering::Relaxed,
                    );
                }
                if restart {
                    if debug_sync {
                        eprintln!(
                            "audio event: restart (playing {}, position {} ms)",
                            current.playing, current.position_ms
                        );
                    }
                    stop_audio_source(&mut source);
                    stop_audio_source(&mut next_pass);
                    if let Some(output) = &output {
                        output.shared.ring.lock().unwrap().clear();
                    }
                    if !current.playing {
                        if let Some(output) = &output {
                            let _ = output.stream.pause();
                        }
                    } else if let Some(launch) = &launcher {
                        let _ = launch(&path, current);
                    } else {
                        if output.is_none() && !output_failed {
                            match open_audio_output() {
                                Ok(opened) => output = Some(opened),
                                Err(error) => {
                                    output_failed = true;
                                    let _ = errors.send(format!(
                                        "Could not open the audio output ({error}). Video playback continues."
                                    ));
                                }
                            }
                        }
                        if let Some(output) = &output {
                            output.shared.gain_bits.store(
                                volume_gain(current.volume, current.muted).to_bits(),
                                Ordering::Relaxed,
                            );
                            source = start_audio_source(
                                &path,
                                &current,
                                current.position_ms,
                                output.sample_rate,
                                output.channels,
                            );
                            cursor_ms = audio_start_position(&current, current.position_ms) as f64;
                            smoothed_lead_ms = 0.0;
                            settled = false;
                            pass_frames = 0;
                            if source.is_none() {
                                let _ = errors
                                    .send("Could not start FFmpeg to decode the audio.".into());
                            } else if let Err(error) = output.stream.play() {
                                let _ = errors.send(format!(
                                    "Could not start the audio output ({error}). Video playback continues."
                                ));
                            }
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_audio_source(&mut source);
                stop_audio_source(&mut next_pass);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let Some(output) = output.as_ref().filter(|_| current.playing) else {
            continue;
        };
        if source.is_none() {
            continue;
        }
        let channels = usize::from(output.channels.max(1));
        let frames_per_ms = f64::from(output.sample_rate) / 1000.0;
        let rate = clock.rate;
        let buffered_frames = output.shared.ring.lock().unwrap().len() / channels;
        let buffered_ms = buffered_frames as f64 / frames_per_ms;
        if buffered_ms >= TARGET_BUFFER_MS {
            thread::sleep(Duration::from_millis(4));
            continue;
        }
        let looping = current.loop_end_ms > current.loop_start_ms;
        if looping
            && next_pass.is_none()
            && current.loop_end_ms as f64 - cursor_ms <= AUDIO_PRESTART_MS * rate
        {
            next_pass = start_audio_source(
                &path,
                &current,
                current.loop_start_ms,
                output.sample_rate,
                output.channels,
            );
        }
        // Media time reaching the speaker right now, and how that compares
        // with the room clock (plus the listener's own offset).
        let latency_ms = output.shared.latency_us.load(Ordering::Relaxed) as f64 / 1000.0;
        let heard_ms = cursor_ms - (buffered_ms + latency_ms) * rate;
        let now = Instant::now();
        let lead_ms =
            clock.lead_ms(heard_ms, now) + f64::from(offset_ms.load(Ordering::Relaxed)) * rate;
        smoothed_lead_ms = if settled {
            smoothed_lead_ms + (lead_ms - smoothed_lead_ms) * 0.2
        } else {
            lead_ms
        };
        if debug_sync && last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            eprintln!(
                "audio sync: lead {lead_ms:+.0} ms (smoothed {smoothed_lead_ms:+.0}), buffered {buffered_ms:.0} ms, device latency {latency_ms:.0} ms"
            );
        }
        let mut feed_frames = (FEED_CHUNK_MS * frames_per_ms) as usize;
        let mut discard = false;
        // The first read waits for FFmpeg to start. Once it has produced
        // audio, line up exactly, then only correct outside the tolerance.
        let aligning = !settled && pass_frames > 0;
        let action = if aligning {
            settled = true;
            initial_alignment(lead_ms)
        } else if settled {
            sync_action(smoothed_lead_ms)
        } else {
            SyncAction::Feed
        };
        match action {
            SyncAction::Feed => {}
            SyncAction::Wait => {
                // Feed nothing; an initial wait is taken in one go.
                let wait_ms = if aligning {
                    lead_ms.min(500.0)
                } else {
                    smoothed_lead_ms.min(20.0)
                };
                thread::sleep(Duration::from_secs_f64((wait_ms / rate / 1000.0).max(0.0)));
                continue;
            }
            SyncAction::Skip(_) => {
                // Whatever is queued is late too: drop it, then skip the rest
                // of the gap in the decoded stream.
                output.shared.ring.lock().unwrap().clear();
                let behind_ms = -(lead_ms + buffered_ms * rate);
                smoothed_lead_ms = 0.0;
                if behind_ms <= 0.0 {
                    continue;
                }
                feed_frames = (behind_ms / rate * frames_per_ms) as usize;
                discard = true;
            }
            SyncAction::Reseek => {
                if debug_sync {
                    eprintln!("audio event: reseek at lead {lead_ms:+.0} ms");
                }
                stop_audio_source(&mut source);
                stop_audio_source(&mut next_pass);
                output.shared.ring.lock().unwrap().clear();
                let position = clock.position_at(now).max(0.0) as u64;
                source = start_audio_source(
                    &path,
                    &current,
                    position,
                    output.sample_rate,
                    output.channels,
                );
                cursor_ms = audio_start_position(&current, position) as f64;
                smoothed_lead_ms = 0.0;
                settled = false;
                pass_frames = 0;
                continue;
            }
        }
        samples.resize(feed_frames.max(1) * channels, 0.0);
        let read_started = Instant::now();
        let read = match source.as_mut() {
            Some(source) => read_samples(source, &mut bytes, &mut samples),
            None => 0,
        };
        if debug_sync && (discard || read_started.elapsed() > Duration::from_millis(50)) {
            eprintln!(
                "audio event: read {} frames in {:.0} ms (discard {discard}, lead {lead_ms:+.0})",
                read / channels,
                read_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let whole = read - read % channels;
        if !discard && whole > 0 {
            output.shared.ring.lock().unwrap().extend(&samples[..whole]);
        }
        cursor_ms += (whole / channels) as f64 / frames_per_ms * rate;
        pass_frames += whole / channels;
        if read < samples.len() {
            if debug_sync {
                eprintln!(
                    "audio event: pass ended at cursor {cursor_ms:.0} ms after {pass_frames} frames (prestarted next: {})",
                    next_pass.is_some()
                );
            }
            // A pass that produced nothing would only spin: give up on it.
            let pass_was_empty = std::mem::take(&mut pass_frames) == 0;
            if pass_was_empty {
                stop_audio_source(&mut source);
                stop_audio_source(&mut next_pass);
                let _ = errors.send("FFmpeg could not decode this clip's audio.".into());
                continue;
            }
            // This pass of the loop is finished; carry on from its start.
            stop_audio_source(&mut source);
            source = next_pass.take().or_else(|| {
                start_audio_source(
                    &path,
                    &current,
                    current.loop_start_ms,
                    output.sample_rate,
                    output.channels,
                )
            });
            // A track that stops short of the loop end is padded with
            // silence so the next pass still begins on time.
            let short_ms = current.loop_end_ms as f64 - cursor_ms;
            if looping && short_ms > 0.0 && short_ms < 500.0 {
                let padding = (short_ms / rate * frames_per_ms) as usize * channels;
                output
                    .shared
                    .ring
                    .lock()
                    .unwrap()
                    .extend(std::iter::repeat_n(0.0, padding));
            }
            cursor_ms = current.loop_start_ms as f64;
            if source.is_none() {
                let _ = errors.send("Could not restart the audio at the loop point.".into());
            }
        }
    }
}
