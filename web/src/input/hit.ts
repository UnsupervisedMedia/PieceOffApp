/** What hit-testing needs to know about one piece, all in table units (source px).
 *  `points` is the piece's closed outline in piece-local coordinates (core's
 *  `piece_outlines`, relative to the piece's target-cell top-left), x,y pairs;
 *  a `subarray` of the full outline buffer is fine (no copy). The piece is drawn
 *  at `(x, y)` turned by `rotation` quarter turns about `(pivotX, pivotY)`, the
 *  cell centre, exactly as the board shader does (rot 1: p -> (-p.y, p.x)). */
export interface PieceGeom {
  id: number;
  x: number;
  y: number;
  /** Quarter turns, 0..3. */
  rotation: number;
  pivotX: number;
  pivotY: number;
  /** Higher = nearer the viewer (the renderer's depth). */
  depth: number;
  points: ArrayLike<number>;
}

/** Table point into the piece's local outline space (undo position, then rotation). */
function toLocal(g: PieceGeom, tx: number, ty: number): [number, number] {
  let lx = tx - g.x - g.pivotX;
  let ly = ty - g.y - g.pivotY;
  const r = g.rotation & 3;
  // Inverse of the shader's forward turn.
  if (r === 1) [lx, ly] = [ly, -lx];
  else if (r === 2) [lx, ly] = [-lx, -ly];
  else if (r === 3) [lx, ly] = [-ly, lx];
  return [lx + g.pivotX, ly + g.pivotY];
}

/** Even-odd point-in-polygon; correct for the non-convex jigsaw contours. */
function inside(p: ArrayLike<number>, x: number, y: number): boolean {
  let hit = false;
  const n = p.length;
  for (let k = 0, j = n - 2; k < n; j = k, k += 2) {
    const xi = p[k], yi = p[k + 1], xj = p[j], yj = p[j + 1];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit;
  }
  return hit;
}

/** Squared distance from (x, y) to the polygon's edges, or Infinity once past `limit2`
 *  on the bounding box (cheap rejection). */
function edgeDist2(p: ArrayLike<number>, x: number, y: number, limit2: number): number {
  let best = Infinity;
  const n = p.length;
  for (let k = 0, j = n - 2; k < n; j = k, k += 2) {
    const ax = p[j], ay = p[j + 1], bx = p[k], by = p[k + 1];
    const dx = bx - ax, dy = by - ay;
    const len2 = dx * dx + dy * dy;
    let t = len2 > 0 ? ((x - ax) * dx + (y - ay) * dy) / len2 : 0;
    t = t < 0 ? 0 : t > 1 ? 1 : t;
    const ex = ax + t * dx - x, ey = ay + t * dy - y;
    const d2 = ex * ex + ey * ey;
    if (d2 < best) best = d2;
  }
  return best <= limit2 ? best : Infinity;
}

function bbox(p: ArrayLike<number>): [number, number, number, number] {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (let k = 0; k < p.length; k += 2) {
    const x = p[k], y = p[k + 1];
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
  }
  return [minX, minY, maxX, maxY];
}

/** The piece under table point (tx, ty).
 *  1. Among pieces whose outline contains the point, the highest `depth` wins;
 *     on equal depth the earlier entry wins (so a topmost-first list needs no depths).
 *  2. Otherwise, if `tolerance` > 0 (table units; the caller converts its screen
 *     tolerance with `px / camera.scale`), the piece whose outline edge is nearest,
 *     within `tolerance`, wins regardless of depth (ties: depth, then order).
 *  Returns the piece id, or null. */
export function hitTest(pieces: PieceGeom[], tx: number, ty: number, tolerance: number): number | null {
  const tol = tolerance > 0 ? tolerance : 0;
  const tol2 = tol * tol;
  let best: PieceGeom | null = null;
  let near: PieceGeom | null = null;
  let nearD2 = Infinity;
  for (const g of pieces) {
    const [lx, ly] = toLocal(g, tx, ty);
    const [minX, minY, maxX, maxY] = bbox(g.points);
    if (lx < minX - tol || ly < minY - tol || lx > maxX + tol || ly > maxY + tol) continue;
    if (inside(g.points, lx, ly)) {
      if (!best || g.depth > best.depth) best = g;
      continue;
    }
    if (best || tol === 0) continue;
    const d2 = edgeDist2(g.points, lx, ly, tol2);
    if (d2 < nearD2 || (d2 === nearD2 && near && g.depth > near.depth)) {
      near = g;
      nearD2 = d2;
    }
  }
  if (best) return best.id;
  return near ? near.id : null;
}
