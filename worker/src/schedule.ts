/** Inputs to the single Durable Object alarm. Times are epoch milliseconds. */
export interface ScheduleInput {
  now: number;
  /** When unflushed changes first appeared, or null if storage is current. */
  dirtySince: number | null;
  flushMs: number;
  /** Next playback deadline from the room core, or -1 for none. */
  deadline: number;
  /** Sockets that have joined. Playback and liveness only matter with some. */
  joinedSockets: number;
  /** How often to look for dead sockets while players are connected. */
  sweepMs: number;
  /** Never schedule closer than this, so a stuck deadline cannot spin. */
  minGapMs: number;
  /** Never schedule further out than this (setAlarm has a date range). */
  maxAheadMs: number;
}

/**
 * The earliest time the room needs waking, or null if it can sleep. An
 * abandoned room that is still "playing" must not wake on every loop wrap, so
 * the playback deadline only counts while someone is connected.
 */
export function alarmTarget(input: ScheduleInput): number | null {
  let target: number | null = null;
  const consider = (time: number): void => {
    target = target === null ? time : Math.min(target, time);
  };
  if (input.dirtySince !== null) consider(input.dirtySince + input.flushMs);
  if (input.joinedSockets > 0) {
    if (input.deadline >= 0) consider(input.deadline);
    consider(input.now + input.sweepMs);
  }
  if (target === null) return null;
  return Math.min(Math.max(target, input.now + input.minGapMs), input.now + input.maxAheadMs);
}

/**
 * Moving an alarm costs a row written, so only set one when none is pending,
 * the pending one has passed, or the new target is earlier. A later pending
 * alarm that is no longer needed just fires and re-arms itself.
 */
export function shouldSetAlarm(current: number | null, target: number, now: number): boolean {
  return current === null || current <= now || target < current;
}
