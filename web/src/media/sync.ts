// Keeps the board's <video> on the room's playback clock.
//
// The room publishes `PlaybackState` stamped with the server clock
// (`changed_at_ms`); every client derives the position from it the way core
// `PlaybackState::position_at` and the desktop's `estimated_position_at` do,
// using the server clock learned from `welcome.server_now_ms`. The desktop
// paces its own decoder against that clock; a browser <video> paces itself,
// so for the short built-in loops a coarse correction is enough: let the
// element play (natively looping), compare it with the room position every
// CHECK_MS, and seek only when it is more than DRIFT_RESYNC_MS away (measured
// the short way round the loop), or outside the loop in/out window. Rate and
// play/pause are applied directly.
import type { PlaybackState } from '../protocol';

/** Seek when the video is further than this from the room position. */
export const DRIFT_RESYNC_MS = 300;
/** How often the drift is checked (also bounds how far past loop out the video runs). */
export const CHECK_MS = 250;

/** The part of HTMLVideoElement this needs. */
export interface VideoLike {
  currentTime: number;
  duration: number;
  paused: boolean;
  seeking: boolean;
  playbackRate: number;
  readyState: number;
  play(): Promise<void> | void;
  pause(): void;
}

export interface SyncTimers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

/** Loop window [start, end) in ms: loop in/out when set, else the whole clip. */
function loopWindow(p: PlaybackState, durationMs: number): [number, number] {
  if (p.loop_end_ms > p.loop_start_ms) return [p.loop_start_ms, p.loop_end_ms];
  return [0, durationMs];
}

/** Where the room's playback is at server time `serverNowMs`, wrapped into the loop. */
export function targetPositionMs(p: PlaybackState, serverNowMs: number, durationMs: number): number {
  let pos = p.position_ms;
  if (p.playing) pos += Math.max(0, serverNowMs - p.changed_at_ms) * p.rate;
  const [start, end] = loopWindow(p, durationMs);
  const span = end - start;
  if (span > 0 && pos >= end) pos = start + ((pos - start) % span);
  return pos;
}

/** `current - target`, the short way round a loop of [start, end). */
export function loopDriftMs(currentMs: number, targetMs: number, start: number, end: number): number {
  const span = end - start;
  let d = currentMs - targetMs;
  if (span > 0) {
    d = ((d % span) + span) % span;
    if (d > span / 2) d -= span;
  }
  return d;
}

export class MediaSync {
  private playback: PlaybackState | null = null;
  private timer: unknown = null;
  private disposed = false;
  private readonly serverNow: () => number;
  private readonly timers: SyncTimers | null;

  constructor(
    private readonly video: VideoLike,
    o: { serverNow: () => number; timers?: SyncTimers | null },
  ) {
    this.serverNow = o.serverNow;
    this.timers =
      o.timers === undefined
        ? { set: (fn, ms) => setTimeout(fn, ms), clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>) }
        : o.timers;
  }

  /** The room's playback changed (welcome, snapshot, playback_changed). */
  apply(p: PlaybackState): void {
    if (this.disposed) return;
    this.playback = p;
    const rate = Math.min(2, Math.max(0.25, Number.isFinite(p.rate) ? p.rate : 1));
    if (this.video.playbackRate !== rate) this.video.playbackRate = rate;
    this.check();
    this.arm();
  }

  /** Compare with the room clock now; seek if too far off; match play/pause. */
  check(): void {
    const p = this.playback;
    const v = this.video;
    if (!p || this.disposed) return;
    if (p.playing && v.paused) this.play();
    else if (!p.playing && !v.paused) v.pause();
    const duration = v.duration * 1000;
    if (v.readyState < 1 || !Number.isFinite(duration) || duration <= 0 || v.seeking) return;
    const target = targetPositionMs(p, this.serverNow(), duration);
    const [start, end] = loopWindow(p, duration);
    const current = v.currentTime * 1000;
    const outside = current < start || current >= end;
    if (outside || Math.abs(loopDriftMs(current, target, start, end)) > DRIFT_RESYNC_MS) {
      v.currentTime = target / 1000;
    }
  }

  /** A user gesture: retry a play() the browser refused earlier. */
  nudge(): void {
    if (this.playback?.playing && this.video.paused) this.play();
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) this.timers?.clear(this.timer);
    this.timer = null;
  }

  private play(): void {
    try {
      const r = this.video.play();
      if (r && typeof r.catch === 'function') r.catch(() => undefined); // autoplay refused: nudge() retries
    } catch {
      // not playable (yet)
    }
  }

  private arm(): void {
    if (!this.timers || this.timer !== null || this.disposed) return;
    this.timer = this.timers.set(() => {
      this.timer = null;
      this.check();
      this.arm();
    }, CHECK_MS);
  }
}
