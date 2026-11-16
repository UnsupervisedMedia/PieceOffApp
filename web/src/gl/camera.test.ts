import { describe, expect, it } from 'vitest';
import { Camera } from './camera';

describe('Camera', () => {
  it('camera_roundtrip_screen_table', () => {
    const cam = new Camera(390, 844);
    cam.fit(480, 320);
    cam.panBy(13.25, -71.5);
    cam.zoomAt(100, 200, 3.7);
    cam.zoomAt(17, 600, 0.31);
    for (const [sx, sy] of [
      [0, 0],
      [390, 844],
      [123.456, 789.01],
      [-50, 2000],
    ]) {
      const [tx, ty] = cam.toTable(sx, sy);
      const [rx, ry] = cam.toScreen(tx, ty);
      expect(Math.abs(rx - sx)).toBeLessThanOrEqual(1e-6);
      expect(Math.abs(ry - sy)).toBeLessThanOrEqual(1e-6);
    }
    for (const [tx, ty] of [
      [0, 0],
      [480, 320],
      [-200.5, 77.125],
    ]) {
      const [sx, sy] = cam.toScreen(tx, ty);
      const [bx, by] = cam.toTable(sx, sy);
      expect(Math.abs(bx - tx)).toBeLessThanOrEqual(1e-6);
      expect(Math.abs(by - ty)).toBeLessThanOrEqual(1e-6);
    }
  });

  it('zoomAt_keeps_anchor_fixed', () => {
    const cam = new Camera(800, 600);
    cam.fit(480, 320);
    for (const [sx, sy, f] of [
      [400, 300, 2],
      [10, 590, 1.5],
      [799, 1, 0.5],
      [250, 250, 7],
    ]) {
      const before = cam.toTable(sx, sy);
      cam.zoomAt(sx, sy, f);
      const after = cam.toTable(sx, sy);
      expect(after[0]).toBeCloseTo(before[0], 6);
      expect(after[1]).toBeCloseTo(before[1], 6);
      const [ax, ay] = cam.toScreen(before[0], before[1]);
      expect(ax).toBeCloseTo(sx, 6);
      expect(ay).toBeCloseTo(sy, 6);
    }
  });

  it('fit_centres_and_contains_the_table', () => {
    const cam = new Camera(390, 844);
    cam.fit(480, 320);
    const [x0, y0] = cam.toScreen(0, 0);
    const [x1, y1] = cam.toScreen(480, 320);
    expect(x0).toBeGreaterThanOrEqual(0);
    expect(x1).toBeLessThanOrEqual(390);
    expect((x0 + x1) / 2).toBeCloseTo(195, 6);
    expect((y0 + y1) / 2).toBeCloseTo(422, 6);
  });

  it('pan_moves_table_with_the_finger', () => {
    const cam = new Camera(800, 600);
    cam.fit(480, 320);
    const before = cam.toScreen(10, 10);
    cam.panBy(5, -3);
    const after = cam.toScreen(10, 10);
    expect(after[0] - before[0]).toBeCloseTo(5, 9);
    expect(after[1] - before[1]).toBeCloseTo(-3, 9);
  });
});
