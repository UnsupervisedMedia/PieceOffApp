use super::*;

pub(crate) struct DecodedFrame {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) struct WebpFrameCursor {
    pub(crate) frames: image::Frames<'static>,
    pub(crate) rgba: Vec<u8>,
    pub(crate) frame_start_ms: u64,
    pub(crate) frame_delay_ms: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct DecoderCommand {
    pub(crate) playing: bool,
    pub(crate) position_ms: u64,
    pub(crate) rate: f32,
    pub(crate) loop_start_ms: u64,
    pub(crate) loop_end_ms: u64,
    pub(crate) resync: bool,
    /// When `position_ms` was read from the room clock.
    pub(crate) issued_at: Instant,
}

pub(crate) struct DecoderStartup {
    pub(crate) path: PathBuf,
    pub(crate) duration_ms: u64,
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Frames per second the FFmpeg backend decodes at.
    pub(crate) fps: u32,
    pub(crate) control_rx: mpsc::Receiver<DecoderCommand>,
    pub(crate) frames: mpsc::SyncSender<DecodedFrame>,
    pub(crate) errors: mpsc::Sender<String>,
    pub(crate) repaint: egui::Context,
    pub(crate) running: Arc<AtomicBool>,
    pub(crate) worker_alive: Arc<AtomicBool>,
}

pub(crate) struct DecoderWorkerGuard(Arc<AtomicBool>);

impl Drop for DecoderWorkerGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) trait MediaDecoderBackend {
    fn start(self, startup: DecoderStartup) -> thread::JoinHandle<()>;
}

pub(crate) struct FfmpegDecoderBackend {
    pub(crate) program: OsString,
    /// How long one frame read may block before FFmpeg is killed as hung.
    pub(crate) stall_timeout: Duration,
}

impl FfmpegDecoderBackend {
    pub(crate) fn new(program: OsString) -> Self {
        Self {
            program,
            stall_timeout: DECODER_STALL_TIMEOUT,
        }
    }
}

/// A healthy FFmpeg answers within milliseconds; a cold seek into a huge
/// file can take a few seconds. Past this a read is stuck (a hung process, a
/// dead network share), and waiting longer would freeze the video and leave
/// the worker deaf to seeks and to the player leaving.
pub(crate) const DECODER_STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Frames of nothing, in a row, after which a video is called empty: FFmpeg
/// exits successfully but decodes no frame, so restarting it would only spin.
const MAX_EMPTY_PASSES: u8 = 3;

/// What a worker is blocked on, shared with its watchdog.
struct ReadWatch {
    /// Process id of the FFmpeg being read; 0 when none.
    pid: std::sync::atomic::AtomicU32,
    /// Milliseconds after `epoch` when the current read began; 0 when idle.
    reading_since_ms: std::sync::atomic::AtomicU64,
    /// Set when the watchdog killed a process, so the worker can say why.
    stalled: AtomicBool,
    finished: AtomicBool,
    epoch: Instant,
}

impl ReadWatch {
    fn begin_read(&self, pid: u32) {
        self.pid.store(pid, Ordering::Release);
        self.reading_since_ms.store(
            (self.epoch.elapsed().as_millis() as u64).max(1),
            Ordering::Release,
        );
    }

    fn end_read(&self) {
        self.reading_since_ms.store(0, Ordering::Release);
    }
}

/// Kill a process by id. FFmpeg is a child we no longer own at this point (the
/// worker thread is blocked reading it), so ask the OS.
fn kill_process(pid: u32) {
    #[cfg(unix)]
    let result = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    let result = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = result;
}

/// Kill FFmpeg whenever the worker has been stuck in one read for `limit`.
fn spawn_read_watchdog(watch: Arc<ReadWatch>, limit: Duration) {
    thread::spawn(move || {
        let poll = (limit / 4).clamp(Duration::from_millis(20), Duration::from_millis(500));
        while !watch.finished.load(Ordering::Acquire) {
            thread::sleep(poll);
            let since = watch.reading_since_ms.load(Ordering::Acquire);
            if since == 0 {
                continue;
            }
            let waited = watch
                .epoch
                .elapsed()
                .saturating_sub(Duration::from_millis(since));
            if waited >= limit {
                let pid = watch.pid.load(Ordering::Acquire);
                if pid != 0 {
                    watch.stalled.store(true, Ordering::Release);
                    kill_process(pid);
                }
                // One kill per stuck read; the next read sets a new start.
                let _ = watch.reading_since_ms.compare_exchange(
                    since,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    });
}

pub(crate) struct AnimatedWebpDecoderBackend;

pub(crate) const MAX_DECODER_RECOVERY_ATTEMPTS: u8 = 2;

pub(crate) fn decoder_recovery_delay(attempt: u8) -> Duration {
    let multiplier = 1_u64 << attempt.saturating_sub(1).min(3);
    Duration::from_millis(100 * multiplier)
}

impl MediaDecoderBackend for FfmpegDecoderBackend {
    fn start(self, startup: DecoderStartup) -> thread::JoinHandle<()> {
        let alive = startup.worker_alive.clone();
        thread::spawn(move || {
            let _guard = DecoderWorkerGuard(alive);
            run_ffmpeg_decoder(self.program, self.stall_timeout, startup);
        })
    }
}

impl MediaDecoderBackend for AnimatedWebpDecoderBackend {
    fn start(self, startup: DecoderStartup) -> thread::JoinHandle<()> {
        let alive = startup.worker_alive.clone();
        thread::spawn(move || {
            let _guard = DecoderWorkerGuard(alive);
            run_animated_webp_decoder(startup);
        })
    }
}

/// Where the shared room clock is in the media, in milliseconds, given the
/// position it had at `anchor`. Workers follow this instead of counting
/// frames or samples, so a slow start is caught up rather than carried.
#[derive(Clone, Copy)]
pub(crate) struct MediaClock {
    pub(crate) anchor: Instant,
    pub(crate) position_ms: f64,
    pub(crate) rate: f64,
    pub(crate) loop_start_ms: f64,
    pub(crate) loop_end_ms: f64,
}

impl MediaClock {
    pub(crate) fn new(
        anchor: Instant,
        position_ms: u64,
        rate: f32,
        loop_start_ms: u64,
        loop_end_ms: u64,
    ) -> Self {
        Self {
            anchor,
            position_ms: position_ms as f64,
            rate: f64::from(rate.clamp(0.25, 2.0)),
            loop_start_ms: loop_start_ms as f64,
            loop_end_ms: loop_end_ms as f64,
        }
    }

    fn span_ms(&self) -> f64 {
        (self.loop_end_ms - self.loop_start_ms).max(0.0)
    }

    pub(crate) fn position_at(&self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.anchor).as_secs_f64() * 1000.0;
        let position = self.position_ms + elapsed * self.rate;
        let span = self.span_ms();
        if span > 0.0 && position >= self.loop_end_ms {
            self.loop_start_ms + (position - self.loop_start_ms) % span
        } else {
            position
        }
    }

    /// How far `media_ms` is ahead of the clock (negative when it is late),
    /// taking the shorter way around the loop.
    pub(crate) fn lead_ms(&self, media_ms: f64, now: Instant) -> f64 {
        let lead = media_ms - self.position_at(now);
        let span = self.span_ms();
        if span <= 0.0 {
            lead
        } else if lead > span / 2.0 {
            lead - span
        } else if lead < -span / 2.0 {
            lead + span
        } else {
            lead
        }
    }
}

/// A frame this late is dropped instead of shown, so the picture catches up.
const LATE_FRAME_DROP_MS: f64 = 80.0;
/// Further behind than this, seek the decoder instead of decoding through.
const DECODER_RESEEK_MS: f64 = 2500.0;
/// Start decoding the next pass of the loop this long before it is needed.
const LOOP_PRESTART_MS: f64 = 1500.0;

pub(crate) fn run_ffmpeg_decoder(
    ffmpeg_program: OsString,
    stall_timeout: Duration,
    startup: DecoderStartup,
) {
    let DecoderStartup {
        path,
        width,
        height,
        fps,
        control_rx,
        frames: tx,
        errors: decoder_error_tx,
        repaint,
        running,
        ..
    } = startup;
    let frame_size = width.saturating_mul(height).saturating_mul(4);
    if frame_size == 0 {
        return;
    }
    let fps = fps.clamp(1, 60);
    let frame_ms = 1000.0 / f64::from(fps);
    let watch = Arc::new(ReadWatch {
        pid: Default::default(),
        reading_since_ms: Default::default(),
        stalled: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        epoch: Instant::now(),
    });
    spawn_read_watchdog(watch.clone(), stall_timeout);
    // Stops the watchdog however the worker returns.
    struct StopWatch(Arc<ReadWatch>);
    impl Drop for StopWatch {
        fn drop(&mut self) {
            self.0.finished.store(true, Ordering::Release);
        }
    }
    let _stop_watch = StopWatch(watch.clone());
    let mut total_frames: u64 = 0;
    let mut frames_at_last_clean_end: Option<u64> = None;
    let mut empty_passes: u8 = 0;
    let start = |timeline: &DecoderCommand| {
        start_ffmpeg_inner(&ffmpeg_program, &path, width, height, timeline, fps)
    };
    let mut buffer = vec![0u8; frame_size];
    let mut timeline = DecoderCommand {
        playing: false,
        position_ms: 0,
        rate: 1.0,
        loop_start_ms: 0,
        loop_end_ms: 0,
        resync: false,
        issued_at: Instant::now(),
    };
    let mut clock = MediaClock::new(Instant::now(), 0, 1.0, 0, 0);
    // Media time of the next frame the current process will produce.
    let mut frame_time_ms = 0.0_f64;
    let mut paused_frame_sent = false;
    let mut recovery_attempts = 0;
    let mut process = start(&timeline);
    // The following pass of the loop, decoded ahead so the wrap is seamless.
    let mut next_pass: Option<(std::process::Child, BufReader<std::process::ChildStdout>)> = None;
    if process.is_none() {
        let _ = decoder_error_tx.send(
            "Could not start FFmpeg. Install FFmpeg and check that this video can be decoded."
                .into(),
        );
        return;
    }
    loop {
        match control_rx.try_recv() {
            Ok(command) => {
                let now = command.issued_at;
                let same_timeline = command.playing == timeline.playing
                    && command.rate == timeline.rate
                    && command.loop_start_ms == timeline.loop_start_ms
                    && command.loop_end_ms == timeline.loop_end_ms;
                clock = MediaClock::new(
                    now,
                    command.position_ms,
                    command.rate,
                    command.loop_start_ms,
                    command.loop_end_ms,
                );
                // A drift check on an unchanged timeline only re-reads the
                // clock; pacing below absorbs small differences on its own.
                let should_seek = !(command.resync && same_timeline)
                    || clock.lead_ms(frame_time_ms, now).abs() > DECODER_RESEEK_MS;
                timeline = command;
                running.store(command.playing, Ordering::Relaxed);
                if should_seek {
                    frame_time_ms = command.position_ms as f64;
                    paused_frame_sent = false;
                    stop_ffmpeg(&mut process);
                    stop_ffmpeg(&mut next_pass);
                    process = start(&command);
                    if process.is_none() {
                        let _ = decoder_error_tx.send(
                            "FFmpeg could not restart at the requested playback position.".into(),
                        );
                        return;
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                stop_ffmpeg(&mut process);
                stop_ffmpeg(&mut next_pass);
                return;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if !timeline.playing && paused_frame_sent {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        let looping = timeline.loop_end_ms > timeline.loop_start_ms;
        let mut lead_ms = 0.0;
        if timeline.playing {
            let now = Instant::now();
            lead_ms = clock.lead_ms(frame_time_ms, now);
            if lead_ms < -DECODER_RESEEK_MS {
                // Too far behind to decode through: jump to the clock.
                timeline.position_ms = clock.position_at(now) as u64;
                frame_time_ms = timeline.position_ms as f64;
                stop_ffmpeg(&mut process);
                stop_ffmpeg(&mut next_pass);
                process = start(&timeline);
                continue;
            }
            if looping
                && next_pass.is_none()
                && timeline.loop_end_ms as f64 - frame_time_ms <= LOOP_PRESTART_MS
            {
                let mut from_loop_start = timeline;
                from_loop_start.position_ms = timeline.loop_start_ms;
                next_pass = start(&from_loop_start);
            }
            if lead_ms > 2.0 {
                // Not due yet. Sleep in short steps so commands stay prompt.
                thread::sleep(Duration::from_secs_f64(
                    (lead_ms / clock.rate / 1000.0).min(0.010),
                ));
                continue;
            }
        }
        let read_result = process.as_mut().and_then(|(child, reader)| {
            watch.begin_read(child.id());
            let result = reader.read_exact(&mut buffer).ok();
            watch.end_read();
            result
        });
        if read_result.is_some() {
            total_frames += 1;
        }
        if read_result.is_none() {
            let ended_cleanly = process
                .as_mut()
                .and_then(|(child, _)| wait_briefly(child))
                .is_some_and(|status| status.success());
            stop_ffmpeg(&mut process);
            if ended_cleanly {
                // A file that ends is normal; one that ends without a single
                // frame, again and again, is not a video we can play.
                if frames_at_last_clean_end == Some(total_frames) {
                    empty_passes += 1;
                    if empty_passes >= MAX_EMPTY_PASSES {
                        let _ = decoder_error_tx.send(
                            "FFmpeg could not decode any frames from this video. The file may be damaged or use an unsupported codec.".into(),
                        );
                        stop_ffmpeg(&mut next_pass);
                        return;
                    }
                } else {
                    empty_passes = 0;
                }
                frames_at_last_clean_end = Some(total_frames);
            }
            if !ended_cleanly {
                if recovery_attempts < MAX_DECODER_RECOVERY_ATTEMPTS {
                    recovery_attempts += 1;
                    thread::sleep(decoder_recovery_delay(recovery_attempts));
                    timeline.position_ms = frame_time_ms as u64;
                    process = start(&timeline);
                    if process.is_some() {
                        continue;
                    }
                }
                let message = if watch.stalled.load(Ordering::Acquire) {
                    "FFmpeg stopped responding while decoding the video. Check the file, or whether it is on a slow or disconnected drive."
                } else {
                    "FFmpeg stopped while decoding the video after recovery retries. Check the file and codec support."
                };
                let _ = decoder_error_tx.send(message.into());
                stop_ffmpeg(&mut next_pass);
                return;
            }
            recovery_attempts = 0;
            // The file ended: continue from the loop start while playing, or
            // step back to the last frame when paused at the very end.
            frame_time_ms = if timeline.playing {
                timeline.loop_start_ms as f64
            } else {
                (frame_time_ms - frame_ms).max(0.0)
            };
            timeline.position_ms = frame_time_ms as u64;
            process = if timeline.playing {
                next_pass.take().or_else(|| start(&timeline))
            } else {
                start(&timeline)
            };
            if process.is_none() {
                let _ = decoder_error_tx
                    .send("FFmpeg could not restart at the video loop point.".into());
                return;
            }
            paused_frame_sent = false;
            continue;
        }
        recovery_attempts = 0;
        if timeline.playing {
            if lead_ms >= -LATE_FRAME_DROP_MS {
                let frame = DecodedFrame {
                    width,
                    height,
                    rgba: buffer.clone(),
                };
                if tx.try_send(frame).is_ok() {
                    repaint.request_repaint();
                }
            }
            frame_time_ms += frame_ms;
            if looping && frame_time_ms >= timeline.loop_end_ms as f64 {
                frame_time_ms = timeline.loop_start_ms as f64;
                timeline.position_ms = timeline.loop_start_ms;
                stop_ffmpeg(&mut process);
                process = next_pass.take().or_else(|| start(&timeline));
                if process.is_none() {
                    let _ = decoder_error_tx
                        .send("FFmpeg could not restart at the video loop point.".into());
                    return;
                }
            }
        } else {
            let frame = DecodedFrame {
                width,
                height,
                rgba: buffer.clone(),
            };
            match tx.try_send(frame) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {
                    // The pending frame will be consumed on this repaint.
                    // A nonblocking send keeps decoder shutdown responsive.
                    paused_frame_sent = true;
                    repaint.request_repaint();
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

/// Default decode rate for callers that do not choose a quality.
pub(crate) const DEFAULT_VIDEO_FPS: u32 = 24;

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn start_ffmpeg(
    path: &Path,
    width: usize,
    height: usize,
    timeline: &DecoderCommand,
) -> Option<(std::process::Child, BufReader<std::process::ChildStdout>)> {
    start_ffmpeg_inner(
        OsStr::new("ffmpeg"),
        path,
        width,
        height,
        timeline,
        DEFAULT_VIDEO_FPS,
    )
}

/// After the pipe closes FFmpeg is exiting; give it a moment to be reaped so a
/// clean end of file is not mistaken for a crash.
pub(crate) fn wait_briefly(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    for _ in 0..25 {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Decode from `timeline.position_ms` as fast as the reader takes frames; the
/// worker paces them against the room clock. A paused timeline yields one
/// frame.
pub(crate) fn start_ffmpeg_inner(
    program: &OsStr,
    path: &Path,
    width: usize,
    height: usize,
    timeline: &DecoderCommand,
    fps: u32,
) -> Option<(std::process::Child, BufReader<std::process::ChildStdout>)> {
    let seek = format!("{:.3}", timeline.position_ms as f64 / 1000.0);
    let mut child = Command::new(program)
        .args(["-v", "error", "-ss", &seek, "-i"])
        .arg(path)
        .args([
            "-an",
            "-vf",
            &format!("scale={width}:{height}:flags=bilinear,fps={fps}"),
        ])
        .args(if timeline.playing {
            Vec::new()
        } else {
            vec!["-frames:v", "1"]
        })
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    Some((child, BufReader::with_capacity(width * height * 4, stdout)))
}

pub(crate) fn stop_ffmpeg(
    process: &mut Option<(std::process::Child, BufReader<std::process::ChildStdout>)>,
) {
    if let Some((child, _)) = process.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *process = None;
}

/// How much detail and motion the video decoder produces. Higher settings
/// cost more CPU and memory per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VideoQuality {
    Low,
    Medium,
    #[default]
    High,
    Ultra,
}

impl VideoQuality {
    pub(crate) const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// Largest decoded frame (width, height) and the frame rate.
    pub(crate) fn limits(self) -> ((u32, u32), u32) {
        match self {
            Self::Low => ((854, 480), 15),
            Self::Medium => ((1280, 720), 24),
            Self::High => ((1920, 1080), 24),
            Self::Ultra => ((1920, 1080), 30),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Low => "Low · 480p, 15 fps",
            Self::Medium => "Medium · 720p, 24 fps",
            Self::High => "High · 1080p, 24 fps",
            Self::Ultra => "Ultra · 1080p, 30 fps",
        }
    }

    pub(crate) fn short_label(self) -> &'static str {
        match self {
            Self::Low => "480p",
            Self::Medium => "720p",
            Self::High => "1080p",
            Self::Ultra => "1080p30",
        }
    }
}

pub(crate) const MAX_VIDEO_DECODE_WIDTH: u32 = 1920;

pub(crate) const MAX_VIDEO_DECODE_HEIGHT: u32 = 1080;

pub(crate) fn video_decode_dimensions(width: u32, height: u32) -> (u32, u32) {
    fit_video_dimensions(
        width,
        height,
        (MAX_VIDEO_DECODE_WIDTH, MAX_VIDEO_DECODE_HEIGHT),
    )
}

/// Scale the frame down, keeping its shape, to fit inside the given bounds.
pub(crate) fn fit_video_dimensions(width: u32, height: u32, max: (u32, u32)) -> (u32, u32) {
    let (max_width, max_height) = max;
    if width == 0 || height == 0 {
        return (width, height);
    }
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    if u64::from(width) * u64::from(max_height) > u64::from(height) * u64::from(max_width) {
        (
            max_width,
            ((u64::from(height) * u64::from(max_width) / u64::from(width)) as u32).max(1),
        )
    } else {
        (
            ((u64::from(width) * u64::from(max_height) / u64::from(height)) as u32).max(1),
            max_height,
        )
    }
}

pub(crate) fn webp_frame_delay_ms(frame: &image::Frame) -> u64 {
    let (numerator, denominator) = frame.delay().numer_denom_ms();
    if denominator == 0 {
        1
    } else {
        (numerator as u64).div_ceil(denominator as u64).max(1)
    }
}

pub(crate) fn webp_frame_rgba(frame: image::Frame, width: usize, height: usize) -> Vec<u8> {
    let image = frame.into_buffer();
    if image.width() as usize == width && image.height() as usize == height {
        image.into_raw()
    } else {
        image::imageops::resize(
            &image,
            width as u32,
            height as u32,
            image::imageops::FilterType::Triangle,
        )
        .into_raw()
    }
}

pub(crate) fn webp_cursor_at(
    path: &Path,
    target_ms: u64,
    width: usize,
    height: usize,
) -> Result<WebpFrameCursor, String> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(file))
        .map_err(|error| format!("Could not open animated WebP: {error}"))?;
    let mut frames = decoder.into_frames();
    let mut frame_start_ms = 0_u64;
    while let Some(frame) = frames.next() {
        let frame = frame.map_err(|error| format!("Could not decode WebP frame: {error}"))?;
        let frame_delay_ms = webp_frame_delay_ms(&frame);
        let frame_end_ms = frame_start_ms.saturating_add(frame_delay_ms);
        if target_ms < frame_end_ms {
            return Ok(WebpFrameCursor {
                frames,
                rgba: webp_frame_rgba(frame, width, height),
                frame_start_ms,
                frame_delay_ms,
            });
        }
        frame_start_ms = frame_end_ms;
    }
    Err("Animated WebP ended before the requested playback position".into())
}

pub(crate) fn retry_decoder_operation<T>(
    mut operation: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let mut last_error = None;
    for attempt in 0..=MAX_DECODER_RECOVERY_ATTEMPTS {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = Some(error);
                if attempt < MAX_DECODER_RECOVERY_ATTEMPTS {
                    thread::sleep(decoder_recovery_delay(attempt + 1));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "Decoder operation failed".into()))
}

pub(crate) fn webp_cursor_next(
    cursor: &mut WebpFrameCursor,
    width: usize,
    height: usize,
) -> Result<bool, String> {
    let Some(frame) = cursor.frames.next() else {
        return Ok(false);
    };
    let frame = frame.map_err(|error| format!("Could not decode WebP frame: {error}"))?;
    cursor.frame_start_ms = cursor.frame_start_ms.saturating_add(cursor.frame_delay_ms);
    cursor.frame_delay_ms = webp_frame_delay_ms(&frame);
    cursor.rgba = webp_frame_rgba(frame, width, height);
    Ok(true)
}

pub(crate) fn normalize_webp_position(
    position_ms: u64,
    loop_start_ms: u64,
    loop_end_ms: u64,
    duration_ms: u64,
) -> u64 {
    let start = loop_start_ms.min(duration_ms.saturating_sub(1));
    let end = if loop_end_ms > start {
        loop_end_ms.min(duration_ms)
    } else {
        duration_ms
    };
    if position_ms < start || position_ms >= end {
        start
    } else {
        position_ms
    }
}

pub(crate) fn run_animated_webp_decoder(startup: DecoderStartup) {
    let DecoderStartup {
        path,
        duration_ms,
        width,
        height,
        control_rx,
        frames: tx,
        errors,
        repaint,
        running,
        ..
    } = startup;
    if duration_ms == 0 || width == 0 || height == 0 {
        let _ = errors.send("Animated WebP metadata did not include a playable duration.".into());
        return;
    }
    let mut timeline = DecoderCommand {
        playing: false,
        position_ms: 0,
        rate: 1.0,
        loop_start_ms: 0,
        loop_end_ms: duration_ms,
        resync: false,
        issued_at: Instant::now(),
    };
    let mut position_ms = 0;
    let mut cursor =
        match retry_decoder_operation(|| webp_cursor_at(&path, position_ms, width, height)) {
            Ok(cursor) => cursor,
            Err(error) => {
                let _ = errors.send(error);
                return;
            }
        };
    let mut frame_sent = false;
    let mut frame_deadline = Instant::now();
    loop {
        let command = if frame_sent && !timeline.playing {
            match control_rx.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        } else if frame_sent && timeline.playing {
            let remaining_ms = cursor
                .frame_start_ms
                .saturating_add(cursor.frame_delay_ms)
                .min(
                    timeline
                        .loop_end_ms
                        .max(cursor.frame_start_ms.saturating_add(1)),
                )
                .saturating_sub(position_ms);
            let rate = timeline.rate.clamp(0.25, 2.0) as f64;
            let wait = if frame_deadline > Instant::now() {
                frame_deadline.saturating_duration_since(Instant::now())
            } else {
                Duration::from_secs_f64((remaining_ms as f64 / 1000.0 / rate).max(0.001))
            };
            match control_rx.recv_timeout(wait) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            None
        };

        if let Some(command) = command {
            let drift = looped_position_distance_ms(
                position_ms,
                command.position_ms,
                command.loop_start_ms,
                command.loop_end_ms,
            );
            let should_seek = !command.resync || drift > 200;
            let was_playing = timeline.playing;
            timeline = command;
            running.store(command.playing, Ordering::Relaxed);
            if should_seek {
                position_ms = normalize_webp_position(
                    command.position_ms,
                    command.loop_start_ms,
                    command.loop_end_ms,
                    duration_ms,
                );
                cursor = match retry_decoder_operation(|| {
                    webp_cursor_at(&path, position_ms, width, height)
                }) {
                    Ok(cursor) => cursor,
                    Err(error) => {
                        let _ = errors.send(error);
                        return;
                    }
                };
                frame_sent = false;
            } else if command.playing && !was_playing {
                let frame_end = cursor
                    .frame_start_ms
                    .saturating_add(cursor.frame_delay_ms)
                    .min(
                        timeline
                            .loop_end_ms
                            .max(cursor.frame_start_ms.saturating_add(1)),
                    );
                let remaining_ms = frame_end.saturating_sub(position_ms);
                frame_deadline = Instant::now()
                    + Duration::from_secs_f64(
                        (remaining_ms as f64 / 1000.0 / command.rate.clamp(0.25, 2.0) as f64)
                            .max(0.001),
                    );
            }
            continue;
        }

        if !frame_sent {
            let frame = DecodedFrame {
                width,
                height,
                rgba: cursor.rgba.clone(),
            };
            match tx.try_send(frame) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {
                    frame_sent = true;
                    repaint.request_repaint();
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            }
            if timeline.playing {
                let frame_end = cursor
                    .frame_start_ms
                    .saturating_add(cursor.frame_delay_ms)
                    .min(
                        timeline
                            .loop_end_ms
                            .max(cursor.frame_start_ms.saturating_add(1)),
                    );
                let remaining_ms = frame_end.saturating_sub(position_ms);
                frame_deadline = Instant::now()
                    + Duration::from_secs_f64(
                        (remaining_ms as f64 / 1000.0 / timeline.rate.clamp(0.25, 2.0) as f64)
                            .max(0.001),
                    );
            }
            continue;
        }

        let frame_end = cursor
            .frame_start_ms
            .saturating_add(cursor.frame_delay_ms)
            .min(
                timeline
                    .loop_end_ms
                    .max(cursor.frame_start_ms.saturating_add(1)),
            );
        position_ms = frame_end;
        if position_ms >= timeline.loop_end_ms && timeline.loop_end_ms > timeline.loop_start_ms {
            position_ms = normalize_webp_position(
                timeline.loop_start_ms,
                timeline.loop_start_ms,
                timeline.loop_end_ms,
                duration_ms,
            );
            cursor =
                match retry_decoder_operation(|| webp_cursor_at(&path, position_ms, width, height))
                {
                    Ok(cursor) => cursor,
                    Err(error) => {
                        let _ = errors.send(error);
                        return;
                    }
                };
        } else {
            match webp_cursor_next(&mut cursor, width, height) {
                Ok(true) => {}
                Ok(false) => {
                    position_ms = normalize_webp_position(
                        timeline.loop_start_ms,
                        timeline.loop_start_ms,
                        timeline.loop_end_ms,
                        duration_ms,
                    );
                    cursor = match retry_decoder_operation(|| {
                        webp_cursor_at(&path, position_ms, width, height)
                    }) {
                        Ok(cursor) => cursor,
                        Err(error) => {
                            let _ = errors.send(error);
                            return;
                        }
                    };
                }
                Err(error) => {
                    cursor = match retry_decoder_operation(|| {
                        webp_cursor_at(&path, position_ms, width, height)
                    }) {
                        Ok(cursor) => cursor,
                        Err(retry_error) => {
                            let _ = errors.send(format!("{error}; retry failed: {retry_error}"));
                            return;
                        }
                    };
                }
            }
        }
        frame_sent = false;
    }
}
