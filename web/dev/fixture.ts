// Dev-only: a fixed local puzzle for the renderer spike and its tests.
//
// Core's `RoomSnapshot::create` refuses more than 400 pieces (the v15 cap), but
// the spike must measure 1000. The snapshot is plain protocol data, and the
// piece contours depend only on the cut style, grid, board size and shape seed
// (derived from the media sha), so this builds the JSON in TS and lets the real
// core `piece_outlines` derive every contour from it. Piece positions are a
// local deterministic scatter into a tray right of the board, like core's
// tray layout in spirit (not byte-identical, which a renderer test does not
// need). Nothing here ships in play.js.
import type { CutStyle, Piece, RoomSnapshot } from '../src/protocol';

export interface DevSnapshotOpts {
  rows: number;
  cols: number;
  sha: string;
  cut: CutStyle;
  width: number;
  height: number;
  protocolVersion?: number;
}

/** Deterministic xorshift shuffle seeded from the grid, like core's scatter. */
function shuffled(count: number, seed: number): number[] {
  const order = Array.from({ length: count }, (_, i) => i);
  let r = seed >>> 0 || 0x9e3779b9;
  for (let i = count - 1; i > 0; i--) {
    r ^= r << 13;
    r >>>= 0;
    r ^= r >>> 17;
    r ^= r << 5;
    r >>>= 0;
    const j = r % (i + 1);
    [order[i], order[j]] = [order[j], order[i]];
  }
  return order;
}

export function devSnapshot(o: DevSnapshotOpts): RoomSnapshot {
  const count = o.rows * o.cols;
  const cw = o.width / o.cols;
  const ch = o.height / o.rows;
  const trayCols = Math.max(1, Math.floor((o.cols * 3) / 5));
  const spacingX = 1.45;
  const spacingY = 1.45;
  const trayX = o.width + cw;
  const trayRows = Math.ceil(count / trayCols);
  const trayY = (o.height - trayRows * spacingY * ch) / 2;
  const order = shuffled(count, (o.rows << 16) ^ o.cols ^ 0x811c9dc5);
  const pieces: Piece[] = [];
  for (let id = 0; id < count; id++) {
    const slot = order[id];
    pieces.push({
      id,
      group_id: id,
      row: Math.floor(id / o.cols),
      column: id % o.cols,
      x: trayX + (slot % trayCols) * spacingX * cw,
      y: trayY + Math.floor(slot / trayCols) * spacingY * ch,
      placed: false,
      held_by: null,
      rotation: 0,
    });
  }
  return {
    protocol_version: o.protocolVersion ?? 0,
    id: 'spike',
    invite_code: 'SPIKE',
    title: 'Spike',
    media_sha256: o.sha,
    media_extension: 'mp4',
    media_available: true,
    width: o.width,
    height: o.height,
    rows: o.rows,
    columns: o.cols,
    pieces,
    playback: {
      playing: true,
      position_ms: 0,
      rate: 1,
      loop_start_ms: 0,
      loop_end_ms: 0,
      changed_at_ms: 0,
    },
    host_id: 'host',
    revision: 0,
    // Zero: core derives the seed from the media sha (`effective_shape_seed`).
    shape_seed: 0,
    queue: [
      {
        sha256: o.sha,
        extension: 'mp4',
        title: 'Spike',
        width: o.width,
        height: o.height,
        duration_ms: 0,
        available: true,
        position_ms: 0,
        loop_start_ms: 0,
        loop_end_ms: 0,
      },
    ],
    current: 0,
    repeat: 'one',
    video_lock: false,
    stats: { elapsed_ms: 0, running_since_ms: null, started: false, finished: false, players: [] },
    holders: [],
    cut_style: o.cut,
    difficulty: { rotation: false, no_hints: false },
    listed: false,
    chat_enabled: true,
  };
}

/** Table bounds that hold the board and the tray, with a margin. */
export function tableBounds(s: RoomSnapshot): { minX: number; minY: number; w: number; h: number } {
  const cw = s.width / s.columns;
  const ch = s.height / s.rows;
  let minX = 0, minY = 0, maxX = s.width, maxY = s.height;
  for (const p of s.pieces) {
    minX = Math.min(minX, p.x);
    minY = Math.min(minY, p.y);
    maxX = Math.max(maxX, p.x + cw);
    maxY = Math.max(maxY, p.y + ch);
  }
  const m = Math.max(cw, ch);
  return { minX: minX - m, minY: minY - m, w: maxX - minX + 2 * m, h: maxY - minY + 2 * m };
}
