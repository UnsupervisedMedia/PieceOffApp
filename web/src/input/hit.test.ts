// @vitest-environment node
import { beforeAll, describe, expect, it } from 'vitest';
import { piece_outlines } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import { devSnapshot } from '../../dev/fixture';
import { buildMesh } from '../gl/mesh';
import { hitTest, type PieceGeom } from './hit';

const SLOW_ORBIT = 'f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80';

beforeAll(async () => {
  await initWasm();
});

/** Axis-aligned rectangle piece, local origin at its top-left. */
function rect(id: number, x: number, y: number, w: number, h: number, depth = 0, rotation = 0): PieceGeom {
  return { id, x, y, rotation, pivotX: w / 2, pivotY: h / 2, depth, points: new Float32Array([0, 0, w, 0, w, h, 0, h]) };
}

describe('hitTest', () => {
  it('outside_returns_null', () => {
    const pieces = [rect(1, 0, 0, 10, 10), rect(2, 100, 0, 10, 10)];
    expect(hitTest(pieces, 50, 5, 0)).toBeNull();
    expect(hitTest(pieces, -1, 5, 0)).toBeNull();
    expect(hitTest([], 0, 0, 100)).toBeNull();
  });

  it('inside_returns_the_piece', () => {
    const pieces = [rect(1, 0, 0, 10, 10), rect(2, 100, 0, 10, 10)];
    expect(hitTest(pieces, 5, 5, 0)).toBe(1);
    expect(hitTest(pieces, 105, 5, 0)).toBe(2);
  });

  it('topmost_wins_on_overlap', () => {
    const low = rect(1, 0, 0, 20, 20, 0.1);
    const high = rect(2, 10, 10, 20, 20, 0.9);
    expect(hitTest([low, high], 15, 15, 0)).toBe(2);
    expect(hitTest([high, low], 15, 15, 0)).toBe(2);
    expect(hitTest([low, high], 5, 5, 0)).toBe(1);
    // Equal depth: the earlier entry (topmost-first order) wins.
    const a = rect(3, 0, 0, 20, 20, 0.5);
    const b = rect(4, 0, 0, 20, 20, 0.5);
    expect(hitTest([a, b], 5, 5, 0)).toBe(3);
    expect(hitTest([b, a], 5, 5, 0)).toBe(4);
  });

  it('inside_beats_tolerance', () => {
    // A point inside a low piece still picks it over a high piece that is only within tolerance.
    const low = rect(1, 0, 0, 10, 10, 0);
    const high = rect(2, 11, 0, 10, 10, 1);
    expect(hitTest([low, high], 9.5, 5, 5)).toBe(1);
  });

  it('tolerance_picks_nearest_piece_within_range', () => {
    const left = rect(1, 0, 0, 10, 10, 1);
    const right = rect(2, 20, 0, 10, 10, 0);
    expect(hitTest([left, right], 13, 5, 0)).toBeNull();
    expect(hitTest([left, right], 13, 5, 5)).toBe(1); // 3 from left, 7 from right
    expect(hitTest([left, right], 17, 5, 5)).toBe(2); // 7 from left, 3 from right (depth does not matter)
    expect(hitTest([left, right], 15, 5, 4)).toBeNull(); // 5 from both, out of range
    expect(hitTest([left, right], 5, 14, 5)).toBe(1); // below, within range
  });

  it('follows_position_and_quarter_turns', () => {
    // 100x20 bar; one quarter turn about its centre (50,10) makes it 20 wide, 100 tall.
    const bar = rect(1, 1000, 1000, 100, 20, 0, 1);
    expect(hitTest([bar], 1050, 1050, 0)).toBe(1);
    expect(hitTest([bar], 1050, 965, 0)).toBe(1);
    expect(hitTest([bar], 1090, 1010, 0)).toBeNull();
    // Shader convention (rot 1: p -> (-p.y, p.x)): local (99, 1) lands at world centre + (9, 49).
    const notch: PieceGeom = {
      id: 9, x: 0, y: 0, rotation: 1, pivotX: 50, pivotY: 10, depth: 0,
      points: new Float32Array([90, 0, 100, 0, 100, 5, 90, 5]), // small square at local top-right
    };
    // local (95, 2.5) - pivot = (45, -7.5) -> (7.5, 45) + pivot = (57.5, 55)
    expect(hitTest([notch], 57.5, 55, 0)).toBe(9);
    expect(hitTest([notch], 42.5, -35, 0)).toBeNull();
    const half = { ...notch, rotation: 2 };
    expect(hitTest([half], 5, 17.5, 0)).toBe(9);
    const three = { ...notch, rotation: 3 };
    expect(hitTest([three], 42.5, -35, 0)).toBe(9);
  });

  describe('real jigsaw outlines', () => {
    const snap = devSnapshot({ rows: 4, cols: 4, sha: SLOW_ORBIT, cut: 'jigsaw', width: 480, height: 480 });
    const cw = snap.width / snap.columns;
    const ch = snap.height / snap.rows;
    let geoms: PieceGeom[] = [];
    let out: { ids: Uint32Array; offsets: Uint32Array; points: Float32Array };

    beforeAll(() => {
      const o = piece_outlines(JSON.stringify(snap));
      out = { ids: o.ids, offsets: o.offsets, points: o.points };
      o.free();
      // Spread pieces far apart so they never overlap.
      geoms = Array.from(out.ids, (id, i) => ({
        id, x: i * 1000, y: 0, rotation: 0, pivotX: cw / 2, pivotY: ch / 2, depth: 0,
        points: out.points.subarray(out.offsets[i], out.offsets[i + 1]),
      }));
    });

    it('point_exactly_in_a_knob', () => {
      let found = 0;
      for (const g of geoms) {
        const p = g.points;
        let k = 0;
        for (let j = 0; j < p.length; j += 2) if (p[j] > p[k]) k = j;
        if (p[k] < cw + 5) continue; // no right-hand knob
        found++;
        // Just inside the knob's tip, well outside the cell rectangle.
        expect(hitTest(geoms, g.x + p[k] - 2, p[k + 1], 0)).toBe(g.id);
        expect(hitTest(geoms, g.x + p[k] + 2, p[k + 1], 0)).toBeNull();
      }
      expect(found).toBeGreaterThan(0);
    });

    it('non_convex_polygon_matches_triangulation', () => {
      // Independent oracle: the earcut triangulation of the full-detail outline.
      const mesh = buildMesh(out, snap, { maxSamples: 1e9 });
      let knob = 0;
      let socket = 0;
      for (let i = 0; i < geoms.length; i++) {
        const g = geoms[i];
        const f0 = mesh.ranges[i * 4 + 2];
        const fn = mesh.ranges[i * 4 + 3];
        const inTris = (x: number, y: number): boolean => {
          for (let f = f0; f < f0 + fn; f += 3) {
            const [a, b, c] = [mesh.fillIndices[f], mesh.fillIndices[f + 1], mesh.fillIndices[f + 2]];
            const ax = mesh.positions[a * 2], ay = mesh.positions[a * 2 + 1];
            const bx = mesh.positions[b * 2], by = mesh.positions[b * 2 + 1];
            const cx = mesh.positions[c * 2], cy = mesh.positions[c * 2 + 1];
            const d1 = (x - bx) * (ay - by) - (ax - bx) * (y - by);
            const d2 = (x - cx) * (by - cy) - (bx - cx) * (y - cy);
            const d3 = (x - ax) * (cy - ay) - (cx - ax) * (y - ay);
            const neg = d1 < 0 || d2 < 0 || d3 < 0;
            const pos = d1 > 0 || d2 > 0 || d3 > 0;
            if (!(neg && pos)) return true;
          }
          return false;
        };
        for (let ly = -ch * 0.4 + 0.37; ly < ch * 1.4; ly += 3.1) {
          for (let lx = -cw * 0.4 + 0.53; lx < cw * 1.4; lx += 3.1) {
            const want = inTris(lx, ly);
            const got = hitTest([g], g.x + lx, ly, 0) === g.id;
            expect(got, `piece ${g.id} at (${lx.toFixed(2)}, ${ly.toFixed(2)})`).toBe(want);
            const inCell = lx >= 0 && ly >= 0 && lx <= cw && ly <= ch;
            if (want && !inCell) knob++;
            if (!want && inCell) socket++;
          }
        }
      }
      // Both kinds of concavity were exercised.
      expect(knob).toBeGreaterThan(50);
      expect(socket).toBeGreaterThan(50);
    });
  });
});
