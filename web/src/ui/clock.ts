// The room's solve clock (RoomSnapshot.stats, ARCHITECTURE.md "Multiplayer
// model"): banked `elapsed_ms` plus the running stretch since
// `running_since_ms` on the server clock. It ticks locally against the server
// clock offset from `welcome`; no per-second messages. One ticker per board,
// shared by the HUD and the players panel.
import type { RoomSnapshot, SolveStats } from '../protocol';

/** Solve time at server time `serverNow`: banked time plus the running stretch. */
export function solveElapsedMs(stats: SolveStats, serverNow: number): number {
  const run = stats.running_since_ms === null ? 0 : Math.max(0, serverNow - stats.running_since_ms);
  return stats.elapsed_ms + run;
}

/** m:ss, or h:mm:ss from an hour. */
export function formatClock(ms: number): string {
  const t = Math.max(0, Math.floor(ms / 1000));
  const s = t % 60;
  const m = Math.floor(t / 60) % 60;
  const hr = Math.floor(t / 3600);
  const ss = String(s).padStart(2, '0');
  return hr > 0 ? `${hr}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

export interface ClockReading {
  /** formatClock of the elapsed time. */
  text: string;
  ms: number;
  running: boolean;
  /** The puzzle is complete: the time is final. */
  finished: boolean;
  started: boolean;
}

export interface ClockSource {
  snapshot(): RoomSnapshot | null;
  serverNow(): number;
}

export interface ClockTimers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

/** Reads the solve clock and wakes its listeners at each whole second while it runs. */
export class SolveClock {
  private timer: unknown = null;
  private readonly cbs: ((r: ClockReading) => void)[] = [];
  private last: ClockReading = { text: '0:00', ms: 0, running: false, finished: false, started: false };

  constructor(
    private readonly src: ClockSource,
    private readonly timers: ClockTimers,
  ) {}

  get reading(): ClockReading {
    return this.last;
  }

  /** Listen (called at once with the current reading). Returns the unsubscribe. */
  onTick(cb: (r: ClockReading) => void): () => void {
    this.cbs.push(cb);
    cb(this.last);
    return () => {
      const i = this.cbs.indexOf(cb);
      if (i >= 0) this.cbs.splice(i, 1);
    };
  }

  /** Re-read the stats (after a welcome, snapshot or a `pieces_changed` with stats). */
  restart(): void {
    if (this.timer !== null) this.timers.clear(this.timer);
    this.timer = null;
    this.tick();
  }

  dispose(): void {
    if (this.timer !== null) this.timers.clear(this.timer);
    this.timer = null;
    this.cbs.length = 0;
  }

  private tick = () => {
    this.timer = null;
    const s = this.src.snapshot();
    if (!s) return;
    const ms = solveElapsedMs(s.stats, this.src.serverNow());
    const running = s.stats.running_since_ms !== null;
    this.last = { text: formatClock(ms), ms, running, finished: s.stats.finished, started: s.stats.started };
    for (const cb of [...this.cbs]) cb(this.last);
    // Wake at the next whole second of the clock.
    if (running) this.timer = this.timers.set(this.tick, 1000 - (ms % 1000) || 1000);
  };
}
