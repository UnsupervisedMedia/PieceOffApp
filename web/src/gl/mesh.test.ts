// @vitest-environment node
import { beforeAll, describe, expect, it } from 'vitest';
import { piece_outlines, type Outlines } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import type { CutStyle, RoomSnapshot } from '../protocol';
import { devSnapshot } from '../../dev/fixture';
import { buildMesh, type Mesh } from './mesh';

const SLOW_ORBIT = 'f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80';
const STYLES: CutStyle[] = ['jigsaw', 'hexagon', 'triangle'];

beforeAll(async () => {
  await initWasm();
});

function fixture(style: CutStyle, rows = 6, cols = 8): { snap: RoomSnapshot; out: Outlines } {
  const snap = devSnapshot({ rows, cols, sha: SLOW_ORBIT, cut: style, width: 480, height: 320 });
  return { snap, out: piece_outlines(JSON.stringify(snap)) };
}

function pieceVerts(mesh: Mesh, i: number): number[][] {
  const v0 = mesh.ranges[i * 4];
  const vn = mesh.ranges[i * 4 + 1];
  const pts: number[][] = [];
  for (let v = v0; v < v0 + vn; v++) pts.push([mesh.positions[v * 2], mesh.positions[v * 2 + 1]]);
  return pts;
}

/** Points of the full outline where the contour turns by more than 40°. */
function corners(pts: Float32Array, a: number, b: number): number[][] {
  const n = (b - a) / 2;
  const out: number[][] = [];
  for (let i = 0; i < n; i++) {
    const p = (k: number) => [pts[a + ((k + n) % n) * 2], pts[a + ((k + n) % n) * 2 + 1]];
    const [px, py] = p(i - 1);
    const [cx, cy] = p(i);
    const [nx, ny] = p(i + 1);
    const a1 = Math.atan2(cy - py, cx - px);
    const a2 = Math.atan2(ny - cy, nx - cx);
    let d = Math.abs(a2 - a1);
    if (d > Math.PI) d = 2 * Math.PI - d;
    if (d > (40 * Math.PI) / 180) out.push([cx, cy]);
  }
  return out;
}

describe('buildMesh', () => {
  it('mesh_vertices_inside_piece_bbox', () => {
    for (const style of STYLES) {
      const { snap, out } = fixture(style);
      const mesh = buildMesh(out, snap, { maxSamples: 1e9 });
      const offsets = out.offsets;
      const pts = out.points;
      expect(mesh.ranges.length).toBe(snap.pieces.length * 4);
      expect(mesh.pieceOfVertex.length).toBe(mesh.positions.length / 2);
      const cw = snap.width / snap.columns;
      const ch = snap.height / snap.rows;
      for (let i = 0; i < snap.pieces.length; i++) {
        let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
        for (let k = offsets[i]; k < offsets[i + 1]; k += 2) {
          minX = Math.min(minX, pts[k]);
          maxX = Math.max(maxX, pts[k]);
          minY = Math.min(minY, pts[k + 1]);
          maxY = Math.max(maxY, pts[k + 1]);
        }
        const v0 = mesh.ranges[i * 4];
        const vn = mesh.ranges[i * 4 + 1];
        expect(vn).toBeGreaterThanOrEqual(3);
        const piece = snap.pieces[i];
        for (let v = v0; v < v0 + vn; v++) {
          const x = mesh.positions[v * 2];
          const y = mesh.positions[v * 2 + 1];
          expect(mesh.pieceOfVertex[v]).toBe(i);
          expect(x).toBeGreaterThanOrEqual(minX - 1e-4);
          expect(x).toBeLessThanOrEqual(maxX + 1e-4);
          expect(y).toBeGreaterThanOrEqual(minY - 1e-4);
          expect(y).toBeLessThanOrEqual(maxY + 1e-4);
          // UVs are the board-normalised source position of the vertex.
          const u = mesh.uvs[v * 2];
          const w = mesh.uvs[v * 2 + 1];
          expect(u).toBeCloseTo((piece.column * cw + x) / snap.width, 5);
          expect(w).toBeCloseTo((piece.row * ch + y) / snap.height, 5);
          expect(u).toBeGreaterThanOrEqual(-1e-5);
          expect(u).toBeLessThanOrEqual(1 + 1e-5);
          expect(w).toBeGreaterThanOrEqual(-1e-5);
          expect(w).toBeLessThanOrEqual(1 + 1e-5);
        }
        // Every fill index stays inside the piece's own vertices.
        const f0 = mesh.ranges[i * 4 + 2];
        const fn = mesh.ranges[i * 4 + 3];
        for (let f = f0; f < f0 + fn; f++) {
          expect(mesh.fillIndices[f]).toBeGreaterThanOrEqual(v0);
          expect(mesh.fillIndices[f]).toBeLessThan(v0 + vn);
        }
      }
    }
  });

  it('mesh_thinning_keeps_corners', () => {
    for (const style of STYLES) {
      const { snap, out } = fixture(style);
      const full = buildMesh(out, snap, { maxSamples: 1e9 });
      const thin = buildMesh(out, snap, { maxSamples: 8 });
      expect(thin.positions.length).toBeLessThan(full.positions.length);
      for (let i = 0; i < snap.pieces.length; i++) {
        const verts = pieceVerts(thin, i);
        const want = corners(out.points, out.offsets[i], out.offsets[i + 1]);
        expect(want.length).toBeGreaterThanOrEqual(3);
        for (const [cx, cy] of want) {
          const hit = verts.some(([x, y]) => Math.abs(x - cx) < 1e-5 && Math.abs(y - cy) < 1e-5);
          expect(hit, `${style} piece ${i} lost corner ${cx},${cy}`).toBe(true);
        }
      }
    }
    // Jigsaw: 316 samples; a cap of 24 needs every 16th sample.
    const { snap, out } = fixture('jigsaw');
    const thin = buildMesh(out, snap, { maxSamples: 24 });
    const n0 = (out.offsets[1] - out.offsets[0]) / 2;
    expect(thin.ranges[1]).toBeLessThanOrEqual(Math.ceil(n0 / 16) + 4);
  });

  it('mesh_triangle_count_scales_with_samples', () => {
    for (const style of STYLES) {
      const { snap, out } = fixture(style);
      let prevTris = Infinity;
      for (const cap of [1e9, 160, 80, 40, 20]) {
        const mesh = buildMesh(out, snap, { maxSamples: cap });
        let tris = 0;
        for (let i = 0; i < snap.pieces.length; i++) {
          const vn = mesh.ranges[i * 4 + 1];
          const fn = mesh.ranges[i * 4 + 3];
          // A simple polygon of n vertices ear-clips into n - 2 triangles.
          expect(fn / 3, `${style} cap ${cap} piece ${i}`).toBe(vn - 2);
          tris += fn / 3;
        }
        expect(tris * 3).toBe(mesh.fillIndices.length);
        expect(tris).toBeLessThanOrEqual(prevTris);
        prevTris = tris;
      }
    }
    // Jigsaw thins hard: full detail is several times the 16th-sample level.
    const { snap, out } = fixture('jigsaw');
    const full = buildMesh(out, snap, { maxSamples: 1e9 }).fillIndices.length;
    const thin = buildMesh(out, snap, { maxSamples: 20 }).fillIndices.length;
    expect(full / thin).toBeGreaterThan(5);
  });

  it('outline_segments_close_each_piece', () => {
    const { snap, out } = fixture('jigsaw', 3, 3);
    const mesh = buildMesh(out, snap, { maxSamples: 40 });
    let segs = 0;
    for (let i = 0; i < snap.pieces.length; i++) segs += mesh.ranges[i * 4 + 1];
    expect(mesh.lineIndices.length).toBe(segs * 2);
  });

  it('fixture_scales_past_core_create_cap', () => {
    const snap = devSnapshot({ rows: 25, cols: 40, sha: SLOW_ORBIT, cut: 'jigsaw', width: 480, height: 320 });
    expect(snap.pieces.length).toBe(1000);
    const out = piece_outlines(JSON.stringify(snap));
    expect(out.ids.length).toBe(1000);
  });
});
