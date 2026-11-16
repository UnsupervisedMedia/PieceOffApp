// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { LOD_DEBOUNCE_MS, LodPolicy, wantedSamples } from './lod';
import { strideFor } from './mesh';

function timers() {
  let now = 0;
  let next = 1;
  const pending = new Map<number, { at: number; fn: () => void }>();
  return {
    pending,
    set(fn: () => void, ms: number) {
      const h = next++;
      pending.set(h, { at: now + ms, fn });
      return h;
    },
    clear(h: unknown) {
      pending.delete(h as number);
    },
    advance(ms: number) {
      now += ms;
      for (const [h, e] of [...pending]) {
        if (e.at <= now) {
          pending.delete(h);
          e.fn();
        }
      }
    },
  };
}

describe('LOD policy', () => {
  it('wanted_samples_scales_with_on_screen_cell_size', () => {
    // Four samples per screen px of the larger cell side, over 2.2.
    expect(wantedSamples(40, 30, 1, null)).toBeCloseTo((40 * 4) / 2.2);
    expect(wantedSamples(40, 30, 0.5, null)).toBeCloseTo((20 * 4) / 2.2);
    expect(wantedSamples(40, 30, 1, 12)).toBe(12);
  });

  it('rebuilds_only_when_the_stride_changes_and_debounces_150ms', () => {
    const t = timers();
    const built: number[] = [];
    const samples0 = 200;
    const lod = new LodPolicy({ samples0, cellW: 40, cellH: 40, timers: t, rebuild: (max) => built.push(max) });
    expect(lod.samplesAt(1)).toBeCloseTo(wantedSamples(40, 40, 1, null));
    lod.built(strideFor(samples0, wantedSamples(40, 40, 1, null)));
    // Same stride at a nearby zoom: nothing scheduled.
    lod.viewChanged(1.01);
    expect(t.pending.size).toBe(0);
    // Zoomed far out: one rebuild, after the debounce, with the latest zoom.
    lod.viewChanged(0.2);
    lod.viewChanged(0.1);
    expect(t.pending.size).toBe(1);
    t.advance(LOD_DEBOUNCE_MS - 1);
    expect(built).toEqual([]);
    t.advance(1);
    expect(built).toEqual([wantedSamples(40, 40, 0.1, null)]);
    expect(LOD_DEBOUNCE_MS).toBe(150);
  });

  it('zooming_back_before_the_debounce_cancels_the_rebuild', () => {
    const t = timers();
    const built: number[] = [];
    const lod = new LodPolicy({ samples0: 200, cellW: 40, cellH: 40, timers: t, rebuild: (m) => built.push(m) });
    lod.built(strideFor(200, lod.samplesAt(1)));
    lod.viewChanged(0.05);
    lod.viewChanged(1);
    expect(t.pending.size).toBe(0);
    t.advance(1000);
    expect(built).toEqual([]);
  });

  it('a_fixed_cap_never_rebuilds_and_dispose_clears_the_timer', () => {
    const t = timers();
    const built: number[] = [];
    const fixed = new LodPolicy({ samples0: 200, cellW: 40, cellH: 40, cap: 8, timers: t, rebuild: (m) => built.push(m) });
    expect(fixed.samplesAt(123)).toBe(8);
    fixed.built(1);
    fixed.viewChanged(0.01);
    expect(t.pending.size).toBe(0);
    const lod = new LodPolicy({ samples0: 200, cellW: 40, cellH: 40, timers: t, rebuild: (m) => built.push(m) });
    lod.built(1);
    lod.viewChanged(0.01);
    expect(t.pending.size).toBe(1);
    lod.dispose();
    expect(t.pending.size).toBe(0);
    lod.viewChanged(0.02);
    expect(t.pending.size).toBe(0);
    expect(built).toEqual([]);
  });
});
