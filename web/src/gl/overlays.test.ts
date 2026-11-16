import { describe, expect, it } from 'vitest';
import { Camera } from './camera';
import { CURSOR_TTL_MS, OverlayLayers, OverlayModel, PING_TTL_MS, fnv1a, playerColor } from './overlays';

function ctx2d() {
  const calls: [string, unknown[]][] = [];
  const target: Record<string, unknown> = {};
  const c = new Proxy(target, {
    get(t, k) {
      if (typeof k !== 'string') return undefined;
      if (k in t) return t[k];
      if (k === 'measureText') return (s: string) => ({ width: s.length * 6 });
      return (...a: unknown[]) => void calls.push([k, a]);
    },
    set(t, k, v) {
      t[k as string] = v;
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
  return { c, calls, texts: () => calls.filter((x) => x[0] === 'fillText').map((x) => x[1][0]) };
}

function frames() {
  let next = 1;
  const pending = new Map<number, FrameRequestCallback>();
  return {
    pending,
    raf: (cb: FrameRequestCallback) => (pending.set(next, cb), next++),
    caf: (h: number) => void pending.delete(h),
    flush() {
      const l = [...pending.values()];
      pending.clear();
      for (const cb of l) cb(0);
    },
  };
}

describe('overlay colours', () => {
  it('fnv1a_matches_core', () => {
    expect(fnv1a('')).toBe(0x811c9dc5);
    expect(fnv1a('a')).toBe(0xe40c292c);
    expect(fnv1a('foobar')).toBe(0xbf9cf968);
    // UTF-8 bytes, like Rust's str::bytes.
    expect(fnv1a('é')).toBe(fnv1a('é'));
    expect(fnv1a('é')).not.toBe(fnv1a('e'));
  });

  it('player_colour_follows_the_name', () => {
    expect(playerColor('Ann')).toBe(playerColor('Ann'));
    expect(playerColor('Ann')).not.toBe(playerColor('Bo'));
    expect(playerColor('Ann')).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
  });
});

describe('OverlayModel', () => {
  it('cursors_and_pings_live_3s', () => {
    const m = new OverlayModel();
    const cursors = new Map([['c1', { name: 'Bo', x: 1, y: 2, at: 1000 }]]);
    expect(m.liveCursors(cursors, 1000 + CURSOR_TTL_MS - 1)).toHaveLength(1);
    expect(m.liveCursors(cursors, 1000 + CURSOR_TTL_MS)).toHaveLength(0);
    expect(cursors.size).toBe(0); // forgotten, not just hidden
    m.addPing('Bo', 5, 6, 2000);
    expect(m.livePings(2000 + PING_TTL_MS - 1)).toHaveLength(1);
    expect(m.livePings(2000 + PING_TTL_MS)).toHaveLength(0);
    expect(CURSOR_TTL_MS).toBe(3000);
    expect(PING_TTL_MS).toBe(3000);
  });
});

describe('OverlayLayers', () => {
  it('draws_table_holders_cursors_and_animates_only_while_something_lives', () => {
    const under = ctx2d();
    const over = ctx2d();
    const f = frames();
    let now = 0;
    const cam = new Camera(300, 200);
    const model = new OverlayModel();
    const cursors = new Map<string, { name: string; x: number; y: number; at: number }>();
    const uc = document.createElement('canvas');
    const oc = document.createElement('canvas');
    const ov = new OverlayLayers(uc, oc, cam, {
      raf: f.raf, caf: f.caf, now: () => now, model, cursors: () => cursors,
      context2d: (c) => (c === uc ? under.c : over.c),
    });
    ov.setScene({
      layout: { tray: { x: 500, y: 0, width: 200, height: 300 }, tray_pad: 10, scene: { min_x: -2000, min_y: -2000, max_x: 3000, max_y: 3000 } },
      boardW: 480, boardH: 320,
    });
    ov.setHolders([{ id: 1, owner: 'Bo', label: 'edges', x: -300, y: 0, width: 100, height: 100 }]);
    f.flush();
    expect(under.texts()).toContain('edges · Bo');
    expect(f.pending.size).toBe(0);
    cursors.set('c1', { name: 'Cy', x: 10, y: 10, at: 0 });
    ov.requestDraw();
    f.flush();
    expect(over.texts()).toContain('Cy');
    expect(f.pending.size).toBe(1); // keeps animating while the cursor lives
    now = 3000;
    f.flush();
    expect(f.pending.size).toBe(0);
    over.calls.length = 0;
    ov.requestDraw();
    f.flush();
    expect(over.texts()).not.toContain('Cy');
    ov.dispose();
    ov.requestDraw();
    expect(f.pending.size).toBe(0);
  });
});
