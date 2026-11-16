import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RoomSnapshot } from '../protocol';
import { snapshot as snap } from '../net/testkit';
import type { Mesh } from './mesh';
import { BoardRenderer, coverUv, type RendererOptions } from './renderer';
import { FLAG_HELD, FLAG_PLACED } from './shaders';

type Call = [string, unknown[]];

/** A WebGL2 context that records every call. `create*` return fresh objects,
 *  status queries pass, constants are distinct numbers. */
function fakeGl(over: Record<string, (...a: unknown[]) => unknown> = {}) {
  const calls: Call[] = [];
  let n = 0;
  const base: Record<string, (...a: unknown[]) => unknown> = {
    getShaderParameter: () => true,
    getProgramParameter: () => true,
    isContextLost: () => false,
    getExtension: () => null,
    getParameter: () => 'fake-gl',
    getUniformLocation: () => ({ u: ++n }),
    createProgram: () => ({ program: ++n }),
    createShader: () => ({ shader: ++n }),
    createTexture: () => ({ texture: ++n }),
    createBuffer: () => ({ buffer: ++n }),
    createVertexArray: () => ({ vao: ++n }),
  };
  const consts = new Map<string, number>();
  const gl = new Proxy({} as Record<string, unknown>, {
    get(_t, k) {
      if (typeof k !== 'string') return undefined;
      if (/^[A-Z0-9_]+$/.test(k)) {
        if (!consts.has(k)) consts.set(k, 0x1000 + consts.size);
        return consts.get(k);
      }
      return (...a: unknown[]) => {
        calls.push([k, a]);
        const f = over[k] ?? base[k];
        return f ? f(...a) : undefined;
      };
    },
  }) as unknown as WebGL2RenderingContext;
  const named = (name: string) => calls.filter((c) => c[0] === name);
  return { gl, calls, named, consts };
}

function frames() {
  let next = 1;
  const pending = new Map<number, FrameRequestCallback>();
  return {
    pending,
    raf: (cb: FrameRequestCallback) => {
      const h = next++;
      pending.set(h, cb);
      return h;
    },
    caf: (h: number) => void pending.delete(h),
    flush(t = 16) {
      const list = [...pending];
      pending.clear();
      for (const [, cb] of list) cb(t);
    },
  };
}

/** A 2x2 puzzle mesh: one triangle per piece is enough for the bookkeeping. */
function mesh(s: RoomSnapshot): Mesh {
  const count = s.pieces.length;
  const positions = new Float32Array(count * 6);
  const pieceOfVertex = new Uint32Array(count * 3);
  const fill = new Uint32Array(count * 3);
  const lines = new Uint32Array(count * 6);
  for (let i = 0; i < count; i++) {
    positions.set([0, 0, 10, 0, 0, 10], i * 6);
    pieceOfVertex.fill(i, i * 3, i * 3 + 3);
    fill.set([i * 3, i * 3 + 1, i * 3 + 2], i * 3);
    lines.set([i * 3, i * 3 + 1, i * 3 + 1, i * 3 + 2, i * 3 + 2, i * 3], i * 6);
  }
  return {
    positions, uvs: new Float32Array(count * 6), pieceOfVertex, fillIndices: fill, lineIndices: lines,
    ranges: new Uint32Array(count * 4), ids: Uint32Array.from(s.pieces.map((p) => p.id)), strides: new Uint8Array(count).fill(1), maxSamples: 99,
  };
}

function media() {
  const video = document.createElement('video');
  const poster = document.createElement('img');
  document.body.append(video, poster);
  poster.hidden = true;
  return { video, poster };
}

function canvas() {
  const c = document.createElement('canvas');
  document.body.append(c);
  return c;
}

function make(o: Partial<RendererOptions> & { gl?: WebGL2RenderingContext | null } = {}) {
  const f = frames();
  const c = canvas();
  const m = media();
  const { gl, ...rest } = o;
  const r = new BoardRenderer(c, m, { raf: f.raf, caf: f.caf, matchMedia: null, getContext: () => gl ?? null, ...rest });
  return { r, f, c, m };
}

afterEach(() => document.body.replaceChildren());

describe('coverUv', () => {
  it('centre_crops_wider_and_taller_media_and_passes_matching_aspect', () => {
    expect(coverUv(100, 100, 200, 100)).toEqual([0.25, 0, 0.75, 1]);
    expect(coverUv(100, 100, 100, 200)).toEqual([0, 0.25, 1, 0.75]);
    expect(coverUv(160, 90, 1920, 1080)).toEqual([0, 0, 1, 1]);
    expect(coverUv(0, 10, 10, 10)).toEqual([0, 0, 1, 1]);
    expect(coverUv(10, 10, 0, 10)).toEqual([0, 0, 1, 1]);
  });
});

describe('BoardRenderer without WebGL', () => {
  it('no_webgl2_takes_the_context_loss_path', () => {
    const { r, m } = make({ gl: null });
    expect(r.contextLost).toBe(true);
    expect(m.poster.hidden).toBe(false);
    const seen: boolean[] = [];
    r.onContextLost((lost) => seen.push(lost));
    expect(seen).toEqual([true]);
    const s = snap(15);
    expect(() => {
      r.setPuzzle(mesh(s), s);
      r.setPiece(0, 5, 5, 1, FLAG_HELD);
      r.requestRender();
    }).not.toThrow();
    r.dispose();
  });

  it('depth_orders_placed_below_loose_below_held_and_pickup_raises', () => {
    const { r } = make({ gl: null });
    const s = snap(15);
    r.setPuzzle(mesh(s), s);
    r.setPiece(0, 0, 0, 0, FLAG_PLACED);
    const placed = r.pieceDepth(0);
    const loose = [1, 2, 3].map((id) => r.pieceDepth(id));
    for (const d of loose) expect(d).toBeGreaterThan(placed);
    r.setPiece(1, 0, 0, 0, FLAG_HELD);
    expect(r.pieceDepth(1)).toBeGreaterThan(Math.max(...[2, 3].map((id) => r.pieceDepth(id))));
    // Put down: still the topmost loose piece (most recently picked up).
    r.setPiece(1, 0, 0, 0, 0);
    expect(r.pieceDepth(1)).toBeGreaterThan(r.pieceDepth(3));
    expect(r.pieceDepth(1)).toBeLessThan(1 / 3 * 2 + 1e-9);
    // Many pickups force a re-rank; the order survives it.
    for (let k = 0; k < 400; k++) {
      r.setPiece(2, 0, 0, 0, FLAG_HELD);
      r.setPiece(2, 0, 0, 0, 0);
      r.setPiece(3, 0, 0, 0, FLAG_HELD);
      r.setPiece(3, 0, 0, 0, 0);
    }
    expect(r.pieceDepth(3)).toBeGreaterThan(r.pieceDepth(2));
    expect(r.pieceDepth(2)).toBeGreaterThan(r.pieceDepth(1));
    expect(r.pieceDepth(99)).toBe(-1);
  });
});

describe('BoardRenderer with a (fake) WebGL2 context', () => {
  it('uploads_only_dirty_pieces_and_everything_past_64', () => {
    const g = fakeGl();
    const { r, f } = make({ gl: g.gl });
    const s = snap(15, { pieces: Array.from({ length: 100 }, (_, id) => ({ id, group_id: id, row: 0, column: 0, x: 0, y: 0, placed: false, held_by: null, rotation: 0 })) });
    r.setPuzzle(mesh(s), s);
    f.flush();
    g.calls.length = 0;
    r.setPiece(5, 1, 2, 0, 0);
    f.flush();
    const one = g.named('texSubImage2D');
    expect(one).toHaveLength(1);
    expect(one[0][1][4]).toBe(2); // two texels: one piece
    expect(one[0][1][9]).toBe(5 * 8); // from the piece's slot in the CPU copy
    g.calls.length = 0;
    for (let i = 0; i < 65; i++) r.setPiece(i, i, i, 0, 0);
    f.flush();
    const all = g.named('texSubImage2D');
    expect(all).toHaveLength(1);
    expect(all[0][1][4]).toBe(512); // whole rows
    r.dispose();
  });

  it('set_piece_during_context_loss_keeps_state_for_the_restore', () => {
    const contexts = [fakeGl(), fakeGl()];
    let k = 0;
    const { r, f, c, m } = make({ getContext: () => contexts[Math.min(k++, 1)].gl });
    const s = snap(15);
    r.setPuzzle(mesh(s), s);
    f.flush();
    const seen: boolean[] = [];
    r.onContextLost((lost) => seen.push(lost));
    const lost = new Event('webglcontextlost', { cancelable: true });
    c.dispatchEvent(lost);
    expect(lost.defaultPrevented).toBe(true);
    expect(r.contextLost).toBe(true);
    expect(m.poster.hidden).toBe(false);
    expect(() => r.setPiece(2, 77, 88, 3, FLAG_HELD)).not.toThrow();
    c.dispatchEvent(new Event('webglcontextrestored'));
    expect(r.contextLost).toBe(false);
    expect(seen).toEqual([true, false]);
    // The new context got the whole CPU state, including the move made while lost.
    const up = contexts[1].named('texImage2D').find((call) => call[1][2] === contexts[1].consts.get('RGBA32F'));
    expect(up).toBeTruthy();
    const data = up![1][8] as Float32Array;
    expect([data[16], data[17], data[18], data[19]]).toEqual([77, 88, 3, FLAG_HELD]);
    r.dispose();
  });

  it('a_restore_that_cannot_link_stays_lost_without_throwing', () => {
    const good = fakeGl();
    const bad = fakeGl({ getProgramParameter: () => false, getProgramInfoLog: () => 'nope' });
    let k = 0;
    const { r, c } = make({ getContext: () => (k++ === 0 ? good.gl : bad.gl) });
    c.dispatchEvent(new Event('webglcontextlost', { cancelable: true }));
    expect(() => c.dispatchEvent(new Event('webglcontextrestored'))).not.toThrow();
    expect(r.contextLost).toBe(true);
    r.dispose();
  });

  it('a_failed_video_upload_keeps_the_poster_and_stops_uploading_video', () => {
    const { m: el } = { m: media() };
    const video = el.video;
    Object.defineProperty(video, 'readyState', { value: 4 });
    Object.defineProperty(video, 'videoWidth', { value: 64 });
    Object.defineProperty(video, 'videoHeight', { value: 32 });
    Object.defineProperty(el.poster, 'complete', { value: true });
    Object.defineProperty(el.poster, 'naturalWidth', { value: 40 });
    Object.defineProperty(el.poster, 'naturalHeight', { value: 20 });
    const g = fakeGl({
      texImage2D: (...a: unknown[]) => {
        if (a.includes(video)) throw new Error('SecurityError: tainted');
      },
      texSubImage2D: (...a: unknown[]) => {
        if (a.includes(video)) throw new Error('SecurityError: tainted');
      },
    });
    const f = frames();
    const c = canvas();
    const r = new BoardRenderer(c, el, { raf: f.raf, caf: f.caf, matchMedia: null, getContext: () => g.gl });
    const s = snap(15);
    r.setPuzzle(mesh(s), s);
    f.flush(); // poster first (no video frame flagged yet)
    video.dispatchEvent(new Event('playing'));
    f.flush(); // poll sees a frame -> upload attempt throws
    f.flush();
    f.flush();
    const videoUploads = g.calls.filter((call) => (call[0] === 'texImage2D' || call[0] === 'texSubImage2D') && call[1].includes(video));
    expect(videoUploads).toHaveLength(1);
    const posterUploads = g.calls.filter((call) => call[0] === 'texImage2D' && call[1].includes(el.poster));
    expect(posterUploads.length).toBeGreaterThanOrEqual(2); // first, and again after the failure
    expect(r.videoFramesUploaded).toBe(0);
    r.dispose();
  });

  it('frame_poll_stops_while_the_video_is_paused_and_restarts_on_playing', () => {
    const g = fakeGl();
    const { r, f, m } = make({ gl: g.gl });
    // jsdom video: no requestVideoFrameCallback, starts paused -> no poll.
    expect(f.pending.size).toBe(1); // the first render only
    f.flush();
    expect(f.pending.size).toBe(0);
    m.video.dispatchEvent(new Event('playing'));
    expect(f.pending.size).toBe(1);
    f.flush();
    expect(f.pending.size).toBe(1); // polling
    m.video.dispatchEvent(new Event('pause'));
    expect(f.pending.size).toBe(0);
    m.video.dispatchEvent(new Event('playing'));
    m.video.dispatchEvent(new Event('ended'));
    expect(f.pending.size).toBe(0);
    r.dispose();
  });

  it('dispose_releases_frames_listeners_and_gl_objects', () => {
    const loseContext = vi.fn();
    const g = fakeGl({ getExtension: (name) => (name === 'WEBGL_lose_context' ? { loseContext } : null) });
    const removePoster = vi.fn();
    const f = frames();
    const c = canvas();
    const m = media();
    const posterRemove = m.poster.removeEventListener.bind(m.poster);
    m.poster.removeEventListener = ((t: string, fn: EventListener) => {
      removePoster(t);
      posterRemove(t, fn);
    }) as typeof m.poster.removeEventListener;
    const listeners = new Set<() => void>();
    const mm = (q: string) => ({
      media: q,
      addEventListener: (_t: string, fn: () => void) => listeners.add(fn),
      removeEventListener: (_t: string, fn: () => void) => listeners.delete(fn),
    }) as unknown as MediaQueryList;
    const r = new BoardRenderer(c, m, { raf: f.raf, caf: f.caf, matchMedia: mm, getContext: () => g.gl });
    const s = snap(15);
    r.setPuzzle(mesh(s), s);
    m.video.dispatchEvent(new Event('playing'));
    expect(f.pending.size).toBeGreaterThan(0);
    expect(listeners.size).toBe(1);
    r.dispose();
    expect(f.pending.size).toBe(0);
    expect(listeners.size).toBe(0);
    expect(removePoster).toHaveBeenCalledWith('load');
    for (const del of ['deleteProgram', 'deleteVertexArray', 'deleteTexture', 'deleteBuffer']) {
      expect(g.named(del).length, del).toBeGreaterThan(0);
    }
    expect(g.named('deleteTexture')).toHaveLength(2);
    expect(g.named('deleteBuffer')).toHaveLength(5);
    // The context itself is given back, after the GL objects.
    expect(loseContext).toHaveBeenCalledTimes(1);
    const order = g.calls.map((c) => c[0]);
    expect(order.lastIndexOf('getExtension')).toBeGreaterThan(order.lastIndexOf('deleteBuffer'));
    r.dispose();
    expect(loseContext).toHaveBeenCalledTimes(1);
    // Nothing runs after dispose.
    r.requestRender();
    m.video.dispatchEvent(new Event('playing'));
    expect(f.pending.size).toBe(0);
  });

  it('a_device_pixel_ratio_change_resizes_the_canvas', () => {
    const g = fakeGl();
    const listeners = new Set<() => void>();
    const queries: string[] = [];
    const mm = (q: string) => {
      queries.push(q);
      return {
        media: q,
        addEventListener: (_t: string, fn: () => void) => listeners.add(fn),
        removeEventListener: (_t: string, fn: () => void) => listeners.delete(fn),
      } as unknown as MediaQueryList;
    };
    const c = canvas();
    Object.defineProperty(c, 'clientWidth', { value: 100 });
    Object.defineProperty(c, 'clientHeight', { value: 50 });
    const old = globalThis.devicePixelRatio;
    Object.defineProperty(globalThis, 'devicePixelRatio', { value: 1, configurable: true, writable: true });
    const f = frames();
    const r = new BoardRenderer(c, media(), { raf: f.raf, caf: f.caf, matchMedia: mm, getContext: () => g.gl });
    expect(c.width).toBe(100);
    (globalThis as { devicePixelRatio: number }).devicePixelRatio = 2;
    for (const fn of [...listeners]) fn();
    expect(c.width).toBe(200);
    expect(r.devicePixelRatio).toBe(2);
    expect(queries[queries.length - 1]).toBe('(resolution: 2dppx)');
    expect(listeners.size).toBe(1); // re-armed for the new ratio, old one dropped
    r.dispose();
    Object.defineProperty(globalThis, 'devicePixelRatio', { value: old, configurable: true, writable: true });
  });
});
