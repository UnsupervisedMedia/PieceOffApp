// @vitest-environment node
import { describe, expect, it } from 'vitest';
import type { PlaybackState } from '../protocol';
import { DRIFT_RESYNC_MS, MediaSync, loopDriftMs, targetPositionMs, type VideoLike } from './sync';

const pb = (o: Partial<PlaybackState> = {}): PlaybackState => ({
  playing: true, position_ms: 0, rate: 1, loop_start_ms: 0, loop_end_ms: 0, changed_at_ms: 1000, ...o,
});

function video(o: Partial<VideoLike> = {}) {
  const v = {
    currentTime: 0, duration: 10, paused: true, seeking: false, playbackRate: 1, readyState: 4,
    plays: 0, pauses: 0, seeks: [] as number[],
    play() { v.plays++; v.paused = false; return Promise.resolve(); },
    pause() { v.pauses++; v.paused = true; },
    ...o,
  };
  return new Proxy(v, {
    set(t, k, val) {
      if (k === 'currentTime') t.seeks.push(val as number);
      (t as Record<string | symbol, unknown>)[k] = val;
      return true;
    },
  });
}

describe('media sync', () => {
  it('target_position_follows_the_room_clock_rate_and_loop', () => {
    expect(targetPositionMs(pb({ playing: false, position_ms: 1234 }), 99_999, 10_000)).toBe(1234);
    expect(targetPositionMs(pb({ position_ms: 500 }), 3000, 10_000)).toBe(2500);
    expect(targetPositionMs(pb({ position_ms: 0, rate: 2 }), 2000, 10_000)).toBe(2000);
    // Whole-clip loop when there is no loop out.
    expect(targetPositionMs(pb(), 1000 + 25_000, 10_000)).toBe(5000);
    // Loop in/out window.
    expect(targetPositionMs(pb({ loop_start_ms: 2000, loop_end_ms: 4000, position_ms: 2000 }), 1000 + 3000, 10_000)).toBe(3000);
    expect(targetPositionMs(pb({ loop_start_ms: 2000, loop_end_ms: 4000, position_ms: 2000 }), 1000 + 2500, 10_000)).toBe(2500 + 0 * 1);
    expect(targetPositionMs(pb({ loop_start_ms: 2000, loop_end_ms: 4000, position_ms: 2000 }), 1000 + 4500, 10_000)).toBe(2500);
    // Clock behind the stamp: never before the position.
    expect(targetPositionMs(pb({ position_ms: 700 }), 0, 10_000)).toBe(700);
  });

  it('drift_takes_the_short_way_round_the_loop', () => {
    expect(loopDriftMs(9900, 100, 0, 10_000)).toBe(-200 + 0);
    expect(loopDriftMs(100, 9900, 0, 10_000)).toBe(200);
    expect(loopDriftMs(5000, 4000, 0, 10_000)).toBe(1000);
  });

  it('resyncs_only_when_drift_exceeds_300ms', () => {
    let now = 1000;
    const v = video({ currentTime: 0.25 });
    const s = new MediaSync(v, { serverNow: () => now, timers: null });
    s.apply(pb()); // target 0, video at 250 ms: inside the window
    expect(v.seeks).toEqual([]);
    expect(v.plays).toBe(1);
    now = 1000 + 2000; // target 2000, video still at 250: 1750 ms behind
    s.check();
    expect(v.seeks).toEqual([2]);
    expect(DRIFT_RESYNC_MS).toBe(300);
  });

  it('applies_pause_rate_and_the_paused_position', () => {
    const v = video({ paused: false, currentTime: 3 });
    const s = new MediaSync(v, { serverNow: () => 50_000, timers: null });
    s.apply(pb({ playing: false, position_ms: 6000, rate: 1.5 }));
    expect(v.pauses).toBe(1);
    expect(v.playbackRate).toBe(1.5);
    expect(v.seeks).toEqual([6]);
    // Rates the desktop clamps stay clamped.
    s.apply(pb({ playing: false, position_ms: 6000, rate: 9 }));
    expect(v.playbackRate).toBe(2);
  });

  it('loop_out_sends_the_video_back_to_loop_in', () => {
    let now = 1000;
    const v = video({ currentTime: 0 });
    const s = new MediaSync(v, { serverNow: () => now, timers: null });
    s.apply(pb({ loop_start_ms: 2000, loop_end_ms: 4000, position_ms: 2000 }));
    expect(v.seeks).toEqual([2]);
    v.currentTime = 4.05; // drifted past loop out by itself
    v.seeks.length = 0;
    now = 1000 + 2050; // room wrapped to 2050
    s.check();
    expect(v.seeks).toEqual([2.05]);
  });

  it('waits_for_metadata_and_skips_while_seeking', () => {
    const v = video({ duration: Number.NaN, readyState: 0 });
    const s = new MediaSync(v, { serverNow: () => 9000, timers: null });
    s.apply(pb());
    expect(v.seeks).toEqual([]);
    v.readyState = 4;
    v.duration = 10;
    v.seeking = true;
    s.check();
    expect(v.seeks).toEqual([]);
    v.seeking = false;
    s.check();
    expect(v.seeks).toEqual([8]);
  });

  it('runs_a_periodic_check_and_dispose_stops_it', () => {
    const pending = new Map<number, () => void>();
    let h = 0;
    const timers = { set: (fn: () => void) => (pending.set(++h, fn), h), clear: (x: unknown) => void pending.delete(x as number) };
    const v = video();
    const s = new MediaSync(v, { serverNow: () => 1000, timers });
    s.apply(pb());
    expect(pending.size).toBe(1);
    const [[k, fn]] = [...pending];
    pending.delete(k);
    fn();
    expect(pending.size).toBe(1); // re-armed
    s.dispose();
    expect(pending.size).toBe(0);
  });
});
