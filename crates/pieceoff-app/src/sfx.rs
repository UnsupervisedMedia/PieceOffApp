//! Short sound effects for room events: a click when pieces snap into
//! place, a softer knock when groups join, a chime when the puzzle is done,
//! and blips for chat and pings. Every sound is synthesised here from sine
//! and triangle partials and a little noise; there are no audio files.
//!
//! Effects play on their own output stream, opened on the first effect, so
//! the clip's audio feeder and its sync logic are never touched. They work
//! the same in rooms whose media is silent or a still image.

use super::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::f32::consts::TAU;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Effect {
    /// Pieces snapped into their place on the board.
    Snap,
    /// Two groups joined into one.
    Join,
    /// The last piece went in.
    Complete,
    /// Someone else said something in chat.
    Chat,
    /// A player marked a spot on the table.
    Ping,
}

impl Effect {
    pub(crate) const ALL: [Effect; 5] = [
        Effect::Snap,
        Effect::Join,
        Effect::Complete,
        Effect::Chat,
        Effect::Ping,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// The same effect is not repeated sooner than this, so a burst of server
/// updates plays once.
pub(crate) const EFFECT_MIN_GAP_SECONDS: f64 = 0.08;
/// Default effects volume (0–100); with the squared slider curve this is
/// about a quarter of full scale.
pub(crate) const DEFAULT_EFFECTS_VOLUME: u8 = 50;
/// Loudest sample any effect buffer may contain, before the volume.
pub(crate) const EFFECT_PEAK: f32 = 0.5;
/// Overlapping effects beyond this drop the oldest.
const MAX_VOICES: usize = 8;

/// Remembers when each effect last played.
#[derive(Default)]
pub(crate) struct EffectGate {
    last: [Option<f64>; Effect::ALL.len()],
}

impl EffectGate {
    /// Whether `effect` may play at `now` (seconds); records it if so.
    pub(crate) fn allow(&mut self, effect: Effect, now: f64) -> bool {
        let last = &mut self.last[effect.index()];
        if last.is_some_and(|last| now - last < EFFECT_MIN_GAP_SECONDS && now >= last) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// What a `PiecesChanged` update did to the pieces, worked out before it is
/// applied to the local snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PieceChange {
    /// Some piece went from loose to placed.
    pub(crate) placed: bool,
    /// Some piece moved into another, existing group.
    pub(crate) joined: bool,
}

/// Compare the incoming pieces with the snapshot they will replace. Only the
/// server sets `placed` and `group_id` (a local drag moves positions only),
/// so a local drop is heard once, when the server confirms it.
pub(crate) fn piece_change(snapshot: &RoomSnapshot, incoming: &[Piece]) -> PieceChange {
    let mut change = PieceChange::default();
    for piece in incoming {
        let Some(before) = snapshot.piece(piece.id) else {
            continue;
        };
        change.placed |= piece.placed && !before.placed;
        // A restart puts every piece back in a group of its own, under its
        // own ID; a join moves pieces under some other piece's group.
        change.joined |= piece.group_id != before.group_id && piece.group_id != piece.id;
    }
    change
}

/// The effect for one applied `PiecesChanged`: the finish outranks the snap
/// that caused it, and a restart is silent.
pub(crate) fn pieces_changed_effect(
    change: PieceChange,
    was_complete: bool,
    now_complete: bool,
    restarted: bool,
) -> Option<Effect> {
    if restarted {
        None
    } else if now_complete && !was_complete {
        Some(Effect::Complete)
    } else if change.placed {
        Some(Effect::Snap)
    } else if change.joined {
        Some(Effect::Join)
    } else {
        None
    }
}

/// The app's handle on effects: level, rate limit and the output thread.
pub(crate) struct SoundEffects {
    pub(crate) volume: u8,
    pub(crate) muted: bool,
    gate: EffectGate,
    gain_bits: Arc<AtomicU32>,
    /// Started on the first audible effect; never when audio is disabled.
    player: Option<mpsc::Sender<Effect>>,
    /// Effects that passed the gate and level checks, for tests.
    #[cfg(test)]
    pub(crate) played: Vec<Effect>,
}

impl SoundEffects {
    pub(crate) fn new(volume: Option<u8>, muted: bool) -> Self {
        let volume = volume.unwrap_or(DEFAULT_EFFECTS_VOLUME).min(100);
        Self {
            volume,
            muted,
            gate: EffectGate::default(),
            gain_bits: Arc::new(AtomicU32::new(volume_gain(volume, muted).to_bits())),
            player: None,
            #[cfg(test)]
            played: Vec::new(),
        }
    }

    pub(crate) fn audible(&self) -> bool {
        !self.muted && self.volume > 0
    }

    /// Apply a new volume or mute to anything playing and to what follows.
    pub(crate) fn set_level(&mut self, volume: u8, muted: bool) {
        self.volume = volume.min(100);
        self.muted = muted;
        self.gain_bits.store(
            volume_gain(self.volume, self.muted).to_bits(),
            Ordering::Relaxed,
        );
    }

    /// Play `effect` unless it just played or effects are silent. `now` is
    /// in seconds on any steady clock (the UI's input time).
    pub(crate) fn play(&mut self, effect: Effect, now: f64) {
        if !self.audible() || !self.gate.allow(effect, now) {
            return;
        }
        #[cfg(test)]
        self.played.push(effect);
        if audio_disabled() {
            return;
        }
        let player = self.player.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel();
            let gain_bits = self.gain_bits.clone();
            let _ = thread::Builder::new()
                .name("pieceoff-effects".into())
                .spawn(move || run_effects_player(receiver, gain_bits));
            sender
        });
        // If the output could not be opened the player has exited and this
        // send fails quietly; the clip's audio reports device problems.
        let _ = player.send(effect);
    }
}

/// One effect rendered as mono PCM at `sample_rate`.
pub(crate) fn synthesize(effect: Effect, sample_rate: u32) -> Vec<f32> {
    let rate = sample_rate.max(8_000) as f32;
    let mut out = match effect {
        // A dry wooden click: a few milliseconds of noise over two quickly
        // damped partials.
        Effect::Snap => {
            let mut noise = Noise(0x9e37_79b9);
            render(rate, 0.07, |t| {
                let body = (TAU * 1_750.0 * t).sin() * 0.6 + (TAU * 920.0 * t).sin() * 0.4;
                let tap = noise.next() * (-t / 0.0025).exp();
                body * (-t / 0.012).exp() + tap * 0.8
            })
        }
        // A rounder, lower knock with a small upward lift.
        Effect::Join => render(rate, 0.12, |t| {
            let lift = 1.0 + 0.08 * (t / 0.12);
            let tone = triangle(520.0 * lift * t) * 0.7 + (TAU * 780.0 * lift * t).sin() * 0.3;
            tone * attack(t, 0.003) * (-t / 0.035).exp()
        }),
        // A rising major arpeggio that rings out.
        Effect::Complete => {
            let notes = [523.25_f32, 659.25, 783.99, 1_046.5];
            render(rate, 1.1, |t| {
                notes
                    .iter()
                    .enumerate()
                    .map(|(index, frequency)| {
                        let local = t - index as f32 * 0.09;
                        if local < 0.0 {
                            return 0.0;
                        }
                        let tone = (TAU * frequency * local).sin() * 0.8
                            + (TAU * 2.0 * frequency * local).sin() * 0.2;
                        tone * attack(local, 0.006) * (-local / 0.28).exp()
                    })
                    .sum()
            })
        }
        // A soft single blip.
        Effect::Chat => render(rate, 0.11, |t| {
            (TAU * 990.0 * t).sin() * attack(t, 0.008) * (-t / 0.03).exp()
        }),
        // Two bright rising blips, unlike anything else here.
        Effect::Ping => render(rate, 0.17, |t| {
            let blip = |start: f32, frequency: f32| {
                let local = t - start;
                if local < 0.0 {
                    0.0
                } else {
                    triangle(frequency * local) * attack(local, 0.003) * (-local / 0.022).exp()
                }
            };
            blip(0.0, 1_568.0) + blip(0.07, 2_093.0)
        }),
    };
    finish(&mut out, rate, effect_level(effect));
    out
}

/// Each effect's peak before the volume; the finish and the chime stand out.
fn effect_level(effect: Effect) -> f32 {
    match effect {
        Effect::Snap => 0.35,
        Effect::Join => 0.3,
        Effect::Complete => EFFECT_PEAK,
        Effect::Chat => 0.25,
        Effect::Ping => 0.3,
    }
}

fn render(rate: f32, seconds: f32, mut wave: impl FnMut(f32) -> f32) -> Vec<f32> {
    let frames = (seconds * rate) as usize;
    (0..frames).map(|frame| wave(frame as f32 / rate)).collect()
}

fn attack(t: f32, seconds: f32) -> f32 {
    (t / seconds).min(1.0)
}

/// A triangle wave with period 1 in `phase`, between -1 and 1.
fn triangle(phase: f32) -> f32 {
    let x = phase.fract();
    4.0 * (x - 0.5).abs() - 1.0
}

/// Normalise to `peak`, start from silence and fade the last few
/// milliseconds to exactly zero so nothing clicks at either end.
fn finish(samples: &mut [f32], rate: f32, peak: f32) {
    let loudest = samples
        .iter()
        .filter(|sample| sample.is_finite())
        .fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()));
    let scale = if loudest > 0.0 { peak / loudest } else { 0.0 };
    let fade = ((0.005 * rate) as usize).clamp(1, samples.len().max(1));
    let len = samples.len();
    for (index, sample) in samples.iter_mut().enumerate() {
        let value = if sample.is_finite() { *sample } else { 0.0 };
        let tail = (len - 1 - index) as f32 / fade as f32;
        *sample = (value * scale * tail.min(1.0)).clamp(-peak, peak);
    }
    if let Some(first) = samples.first_mut() {
        *first = 0.0;
    }
}

/// A tiny deterministic noise source (xorshift), so the click is the same
/// every time and nothing random is needed.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

struct Voice {
    samples: Arc<[f32]>,
    next: usize,
}

struct Mixer {
    voices: Mutex<Vec<Voice>>,
    gain_bits: Arc<AtomicU32>,
}

/// Sum the playing voices into the device buffer, the same signal on every
/// channel. Never blocks the audio thread: a contended mixer plays silence.
fn mix<T>(data: &mut [T], channels: usize, mixer: &Mixer)
where
    T: cpal::Sample + cpal::FromSample<f32>,
{
    let gain = f32::from_bits(mixer.gain_bits.load(Ordering::Relaxed));
    let mut voices = mixer.voices.try_lock().ok();
    for frame in data.chunks_mut(channels.max(1)) {
        let mut value = 0.0;
        if let Some(voices) = voices.as_mut() {
            for voice in voices.iter_mut() {
                if let Some(sample) = voice.samples.get(voice.next) {
                    value += sample;
                    voice.next += 1;
                }
            }
        }
        let value = (value * gain).clamp(-1.0, 1.0);
        for sample in frame {
            *sample = T::from_sample(value);
        }
    }
    if let Some(voices) = voices.as_mut() {
        voices.retain(|voice| voice.next < voice.samples.len());
    }
}

fn build_effects_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mixer: Arc<Mixer>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| mix(data, channels, &mixer),
        |_error| {},
        None,
    )
}

/// Owns the effects stream (which must stay on the thread that made it),
/// renders every effect once for the device's rate, and starts voices.
fn run_effects_player(effects: mpsc::Receiver<Effect>, gain_bits: Arc<AtomicU32>) {
    let Ok(device) = cpal::default_host()
        .default_output_device()
        .ok_or(())
        .and_then(|device| {
            device
                .default_output_config()
                .map(|c| (device, c))
                .map_err(|_| ())
        })
    else {
        return;
    };
    let (device, supported) = device;
    let config = supported.config();
    let buffers: Vec<Arc<[f32]>> = Effect::ALL
        .iter()
        .map(|effect| Arc::from(synthesize(*effect, config.sample_rate)))
        .collect();
    let mixer = Arc::new(Mixer {
        voices: Mutex::new(Vec::new()),
        gain_bits,
    });
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_effects_stream::<f32>(&device, config, mixer.clone()),
        cpal::SampleFormat::I16 => build_effects_stream::<i16>(&device, config, mixer.clone()),
        cpal::SampleFormat::U16 => build_effects_stream::<u16>(&device, config, mixer.clone()),
        cpal::SampleFormat::I32 => build_effects_stream::<i32>(&device, config, mixer.clone()),
        cpal::SampleFormat::U8 => build_effects_stream::<u8>(&device, config, mixer.clone()),
        _ => return,
    };
    let Ok(stream) = stream else {
        return;
    };
    if stream.play().is_err() {
        return;
    }
    while let Ok(effect) = effects.recv() {
        let mut voices = mixer.voices.lock().unwrap();
        if voices.len() >= MAX_VOICES {
            voices.remove(0);
        }
        voices.push(Voice {
            samples: buffers[effect.index()].clone(),
            next: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_is_short_quiet_finite_and_ends_in_silence() {
        for rate in [44_100, 48_000, 96_000] {
            for effect in Effect::ALL {
                let samples = synthesize(effect, rate);
                let seconds = samples.len() as f32 / rate as f32;
                assert!(
                    (0.05..=1.2).contains(&seconds),
                    "{effect:?} lasts {seconds} s"
                );
                assert!(samples.iter().all(|s| s.is_finite()), "{effect:?} NaN");
                let peak = samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
                assert!(
                    peak <= EFFECT_PEAK + f32::EPSILON && peak > 0.05,
                    "{effect:?} peak {peak}"
                );
                assert_eq!(samples[0], 0.0, "{effect:?} starts silent");
                assert_eq!(*samples.last().unwrap(), 0.0, "{effect:?} ends silent");
                // The fade-out really fades: the last 2 ms are near silence.
                let tail = &samples[samples.len() - (rate as usize / 500)..];
                assert!(tail.iter().all(|s| s.abs() < 0.2 * peak), "{effect:?} tail");
            }
        }
    }

    #[test]
    fn effects_sound_different_from_each_other() {
        let rendered: Vec<Vec<f32>> = Effect::ALL
            .iter()
            .map(|effect| synthesize(*effect, 48_000))
            .collect();
        for (i, a) in rendered.iter().enumerate() {
            for b in &rendered[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // Rendering is deterministic.
        assert_eq!(synthesize(Effect::Snap, 48_000), rendered[0]);
    }

    #[test]
    fn the_gate_lets_each_effect_through_once_per_gap() {
        let mut gate = EffectGate::default();
        assert!(gate.allow(Effect::Snap, 1.0));
        assert!(!gate.allow(Effect::Snap, 1.03));
        assert!(
            gate.allow(Effect::Join, 1.03),
            "effects are gated separately"
        );
        assert!(gate.allow(Effect::Snap, 1.0 + EFFECT_MIN_GAP_SECONDS));
        // A clock that went backwards (a new egui context) does not mute.
        assert!(gate.allow(Effect::Snap, 0.5));
    }

    #[test]
    fn muted_or_zero_volume_plays_nothing_and_levels_are_clamped() {
        let mut effects = SoundEffects::new(Some(250), false);
        assert_eq!(effects.volume, 100);
        effects.set_level(40, true);
        effects.play(Effect::Chat, 1.0);
        effects.set_level(0, false);
        effects.play(Effect::Chat, 2.0);
        assert!(effects.played.is_empty());
        effects.set_level(40, false);
        effects.play(Effect::Chat, 3.0);
        effects.play(Effect::Chat, 3.01);
        assert_eq!(effects.played, vec![Effect::Chat]);
        assert!(effects.player.is_none(), "tests never open an output");
        assert_eq!(
            SoundEffects::new(None, false).volume,
            DEFAULT_EFFECTS_VOLUME
        );
    }

    fn piece(id: u32, group_id: u32, placed: bool) -> Piece {
        Piece {
            id,
            group_id,
            row: 0,
            column: id as u16,
            x: 0.0,
            y: 0.0,
            placed,
            held_by: None,
        }
    }

    #[test]
    fn piece_changes_are_classified_from_the_server_flags() {
        let mut snapshot = RoomSnapshot::create(
            "room".into(),
            "INVITE".into(),
            "t".into(),
            "a".repeat(64),
            "png".into(),
            400,
            300,
            2,
            2,
            "host".into(),
        )
        .unwrap();
        for (index, p) in snapshot.pieces.iter_mut().enumerate() {
            *p = piece(index as u32, index as u32, false);
        }
        // Moving pieces is silent.
        let mut moved = piece(0, 0, false);
        moved.x = 50.0;
        assert_eq!(piece_change(&snapshot, &[moved]), PieceChange::default());
        // Piece 1 joins piece 0's group.
        let joined = piece_change(&snapshot, &[piece(1, 0, false)]);
        assert!(joined.joined && !joined.placed);
        // Pieces placed.
        let placed = piece_change(&snapshot, &[piece(2, 2, true)]);
        assert!(placed.placed);
        // A restart puts every piece back in its own group: not a join.
        snapshot.pieces[1].group_id = 0;
        snapshot.pieces[2].placed = true;
        let restart: Vec<Piece> = (0..4).map(|id| piece(id, id, false)).collect();
        assert_eq!(piece_change(&snapshot, &restart), PieceChange::default());
    }

    #[test]
    fn the_finish_outranks_a_snap_and_a_restart_is_silent() {
        let both = PieceChange {
            placed: true,
            joined: true,
        };
        assert_eq!(
            pieces_changed_effect(both, false, true, false),
            Some(Effect::Complete)
        );
        assert_eq!(
            pieces_changed_effect(both, false, false, false),
            Some(Effect::Snap)
        );
        assert_eq!(
            pieces_changed_effect(
                PieceChange {
                    placed: false,
                    joined: true
                },
                false,
                false,
                false
            ),
            Some(Effect::Join)
        );
        // An update to a finished puzzle does not chime again.
        assert_eq!(
            pieces_changed_effect(PieceChange::default(), true, true, false),
            None
        );
        assert_eq!(pieces_changed_effect(both, true, false, true), None);
    }
}
