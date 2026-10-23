/** Inputs to the room's expiry. Times are epoch milliseconds. */
export interface CleanupInput {
  /** Players currently connected. Any activity keeps the room alive. */
  joinedSockets: number;
  /** When the room last became empty (or was created), or null if unset. */
  emptySince: number | null;
  /** How long an empty room is kept. */
  idleMs: number;
  /** The creator has never connected: they may have lost the link. */
  unclaimed: boolean;
  /** How long a never-claimed room is kept; capped by `idleMs`. */
  unclaimedMs: number;
}

/**
 * When an empty room should be deleted, or null if it should stay. A room
 * with someone in it never expires; an empty one expires `idleMs` after the
 * last player left, or sooner if its creator never showed up.
 */
export function cleanupDeadline(input: CleanupInput): number | null {
  if (input.joinedSockets > 0 || input.emptySince === null) return null;
  const keep = input.unclaimed ? Math.min(input.unclaimedMs, input.idleMs) : input.idleMs;
  return input.emptySince + keep;
}
