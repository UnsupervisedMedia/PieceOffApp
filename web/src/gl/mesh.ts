import earcut from 'earcut';
import type { RoomSnapshot } from '../protocol';

/** Just what the mesh needs from WASM `piece_outlines` (structural, so tests
 *  and callers can hand in either the wasm object or plain arrays). */
export interface OutlineSource {
  readonly ids: Uint32Array;
  readonly offsets: Uint32Array;
  readonly points: Float32Array;
}

/** Every piece of a puzzle in one batch. Positions are piece-local source pixels
 *  (relative to the piece's target-cell top-left, as core's outlines are); the
 *  renderer adds each piece's position/rotation from its state texture.
 *  `uvs` are board-normalised (0..1 over the board); the media crop is a
 *  shader uniform so a media size change needs no rebuild. */
export interface Mesh {
  positions: Float32Array;
  uvs: Float32Array;
  /** Snapshot piece index (not id) of each vertex. */
  pieceOfVertex: Uint32Array;
  /** Triangles, three indices each. */
  fillIndices: Uint32Array;
  /** Closed outline per piece as line-segment pairs. */
  lineIndices: Uint32Array;
  /** Per piece: first vertex, vertex count, first fill index, fill index count. */
  ranges: Uint32Array;
  /** Piece id per index. */
  ids: Uint32Array;
  /** Sample stride used per piece (1 = full detail). */
  strides: Uint8Array;
  maxSamples: number;
}

/** Thinning factors, finest first (desktop's DETAIL_STRIDES). */
export const DETAIL_STRIDES = [1, 2, 4, 8, 16] as const;
/** Turn above which a sample is a corner and survives any thinning. */
const CORNER_TURN = (30 * Math.PI) / 180;

function cornerMask(pts: Float32Array, a: number, n: number): Uint8Array {
  const mask = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    const p = a + ((i + n - 1) % n) * 2;
    const c = a + i * 2;
    const q = a + ((i + 1) % n) * 2;
    const a1 = Math.atan2(pts[c + 1] - pts[p + 1], pts[c] - pts[p]);
    const a2 = Math.atan2(pts[q + 1] - pts[c + 1], pts[q] - pts[c]);
    let d = Math.abs(a2 - a1);
    if (d > Math.PI) d = 2 * Math.PI - d;
    if (d > CORNER_TURN) mask[i] = 1;
  }
  return mask;
}

function thin(pts: Float32Array, a: number, n: number, mask: Uint8Array, stride: number): number[] {
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    if (i % stride === 0 || mask[i]) out.push(pts[a + i * 2], pts[a + i * 2 + 1]);
  }
  return out;
}

/** Coarsest stride whose sample count fits `maxSamples`. */
export function strideFor(n: number, maxSamples: number): number {
  for (const s of DETAIL_STRIDES) if (Math.ceil(n / s) <= maxSamples) return s;
  return DETAIL_STRIDES[DETAIL_STRIDES.length - 1];
}

/** Triangulate every piece into one batch. Pieces whose full outline has more
 *  than `maxSamples` points use every 2nd..16th sample (corners always kept),
 *  falling back to a finer level if the thinned polygon will not ear-clip
 *  cleanly. */
export function buildMesh(out: OutlineSource, snapshot: RoomSnapshot, opts: { maxSamples: number }): Mesh {
  const ids = out.ids;
  const offsets = out.offsets;
  const pts = out.points;
  const count = ids.length;
  const cw = snapshot.width / snapshot.columns;
  const ch = snapshot.height / snapshot.rows;
  const byId = new Map(snapshot.pieces.map((p) => [p.id, p]));

  const pos: number[][] = [];
  const tris: number[][] = [];
  const strides = new Uint8Array(count);
  let vTotal = 0;
  let fTotal = 0;
  for (let i = 0; i < count; i++) {
    const a = offsets[i];
    const n = (offsets[i + 1] - a) / 2;
    const mask = cornerMask(pts, a, n);
    let s = strideFor(n, opts.maxSamples);
    let poly: number[] = [];
    let tri: number[] = [];
    for (;;) {
      poly = thin(pts, a, n, mask, s);
      tri = earcut(poly);
      if (tri.length === (poly.length / 2 - 2) * 3 || s === 1) break;
      s >>= 1;
    }
    strides[i] = s;
    pos.push(poly);
    tris.push(tri);
    vTotal += poly.length / 2;
    fTotal += tri.length;
  }

  const positions = new Float32Array(vTotal * 2);
  const uvs = new Float32Array(vTotal * 2);
  const pieceOfVertex = new Uint32Array(vTotal);
  const fillIndices = new Uint32Array(fTotal);
  const lineIndices = new Uint32Array(vTotal * 2);
  const ranges = new Uint32Array(count * 4);
  let v = 0;
  let f = 0;
  for (let i = 0; i < count; i++) {
    const piece = byId.get(ids[i]);
    const baseX = piece ? piece.column * cw : 0;
    const baseY = piece ? piece.row * ch : 0;
    const poly = pos[i];
    const tri = tris[i];
    const vn = poly.length / 2;
    ranges[i * 4] = v;
    ranges[i * 4 + 1] = vn;
    ranges[i * 4 + 2] = f;
    ranges[i * 4 + 3] = tri.length;
    for (let k = 0; k < vn; k++) {
      const x = poly[k * 2];
      const y = poly[k * 2 + 1];
      positions[(v + k) * 2] = x;
      positions[(v + k) * 2 + 1] = y;
      uvs[(v + k) * 2] = (baseX + x) / snapshot.width;
      uvs[(v + k) * 2 + 1] = (baseY + y) / snapshot.height;
      pieceOfVertex[v + k] = i;
      lineIndices[(v + k) * 2] = v + k;
      lineIndices[(v + k) * 2 + 1] = v + ((k + 1) % vn);
    }
    for (let k = 0; k < tri.length; k++) fillIndices[f + k] = v + tri[k];
    v += vn;
    f += tri.length;
  }
  return {
    positions,
    uvs,
    pieceOfVertex,
    fillIndices,
    lineIndices,
    ranges,
    ids: Uint32Array.from(ids),
    strides,
    maxSamples: opts.maxSamples,
  };
}
