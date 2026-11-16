// Turned pieces (protocol v16: `Piece.rotation`, `rotate_selection`) are not on
// this branch's generated protocol (v15). Everything rotation-shaped goes
// through here so it switches on by itself once the types are regenerated:
// `HAS_ROTATION` comes from the generated tag list, `RotateSelection` is the
// generated variant as soon as it exists (so the field names are then checked
// against it), and pieces are read through `pieceRotation`.
import { CLIENT_MESSAGE_TYPES, type ClientMessage, type Piece, type RoomSnapshot } from '../protocol';

/** The generated protocol has `rotate_selection`. */
export const HAS_ROTATION: boolean = (CLIENT_MESSAGE_TYPES as readonly string[]).includes('rotate_selection');

type Generated = Extract<ClientMessage, { type: 'rotate_selection' }>;
/** The generated variant once it exists; a stand-in of the v16 shape until then. */
export type RotateSelection = [Generated] extends [never]
  ? { type: 'rotate_selection'; anchor_piece_id: number; quarter_turns: number }
  : Generated;

/** Clockwise quarter turns 0..3 (0 on protocols without rotation). */
export function pieceRotation(p: Piece): number {
  const r = (p as { rotation?: number }).rotation ?? 0;
  return ((r % 4) + 4) % 4;
}

/** The room turns its pieces (`snapshot.difficulty.rotation`, v16). */
export function roomRotates(s: RoomSnapshot | null): boolean {
  if (!HAS_ROTATION || !s) return false;
  return (s as { difficulty?: { rotation?: boolean } }).difficulty?.rotation === true;
}

/** `rotate_selection` for what the player holds, or null where the protocol has none. */
export function rotateMessage(anchorPieceId: number, quarterTurns: 1 | -1): ClientMessage | null {
  if (!HAS_ROTATION) return null;
  const m: RotateSelection = { type: 'rotate_selection', anchor_piece_id: anchorPieceId, quarter_turns: quarterTurns } as RotateSelection;
  return m as unknown as ClientMessage;
}
