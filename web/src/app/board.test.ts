import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { protocol_version } from '../../wasm/pieceoff_web_wasm.js';
import { Camera } from '../gl/camera';
import type { Mesh } from '../gl/mesh';
import { FLAG_HELD, FLAG_SELECTED } from '../gl/shaders';
import { RoomState } from '../net/roomState';
import { snapshot as baseSnapshot, welcome } from '../net/testkit';
import type { ClientMessage, Piece, RoomSnapshot, ServerMessage } from '../protocol';
import { initWasm } from '../test-wasm';
import { formatClock, mountBoard, type BoardHandle, type RendererLike } from './board';
import { BUILTIN_MEDIA } from './create';
import { HAS_ROTATION } from './rotation';

let V: number;
beforeAll(async () => {
  await initWasm();
  V = protocol_version();
});

// ---- fakes -------------------------------------------------------------------

class FakeRenderer implements RendererLike {
  readonly camera: Camera;
  readonly calls: [number, number, number, number, number][] = [];
  puzzles = 0;
  renders = 0;
  disposed = false;
  contextLost = false;
  private cbs: ((lost: boolean) => void)[] = [];
  constructor(camera: Camera) {
    this.camera = camera;
  }
  setPuzzle(_m: Mesh, _s: RoomSnapshot): void {
    this.puzzles++;
  }
  setPiece(id: number, x: number, y: number, rot: number, flags: number): void {
    this.calls.push([id, x, y, rot, flags]);
  }
  pieceDepth(id: number): number {
    return id / 1000;
  }
  requestRender(): void {
    this.renders++;
  }
  onContextLost(cb: (lost: boolean) => void): void {
    this.cbs.push(cb);
  }
  resize(): void {}
  dispose(): void {
    this.disposed = true;
  }
  lose(lost: boolean): void {
    this.contextLost = lost;
    for (const cb of this.cbs) cb(lost);
  }
  last(id: number) {
    const all = this.calls.filter((c) => c[0] === id);
    return all[all.length - 1];
  }
}

function clock() {
  let now = 0;
  let next = 1;
  const pending = new Map<number, { at: number; fn: () => void }>();
  const frames = new Map<number, FrameRequestCallback>();
  const c = {
    get now() {
      return now;
    },
    pending,
    frames,
    timers: {
      set(fn: () => void, ms: number) {
        const h = next++;
        pending.set(h, { at: now + ms, fn });
        return h;
      },
      clear(h: unknown) {
        pending.delete(h as number);
      },
    },
    raf: (cb: FrameRequestCallback) => {
      const h = next++;
      frames.set(h, cb);
      return h;
    },
    caf: (h: number) => void frames.delete(h),
    flushFrames() {
      const l = [...frames.values()];
      frames.clear();
      for (const cb of l) cb(now);
    },
    advance(ms: number) {
      const end = now + ms;
      for (;;) {
        let best: [number, { at: number; fn: () => void }] | null = null;
        for (const e of pending) if (e[1].at <= end && (!best || e[1].at < best[1].at)) best = e;
        if (!best) break;
        pending.delete(best[0]);
        now = best[1].at;
        best[1].fn();
      }
      now = end;
      c.flushFrames();
    },
  };
  return c;
}

function ctx2d() {
  const calls: [string, unknown[]][] = [];
  const target: Record<string, unknown> = {};
  const ctx = new Proxy(target, {
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
  return { ctx, calls, texts: () => calls.filter((x) => x[0] === 'fillText').map((x) => x[1][0] as string) };
}

/** Four loose pieces well apart (no overlap), in snapshot order. */
function spreadPieces(): Piece[] {
  return Array.from({ length: 4 }, (_, id) => ({
    id, group_id: id, row: Math.floor(id / 2), column: id % 2,
    x: 600 + id * 400, y: 100, placed: false, held_by: null,
  }));
}

interface Setup {
  h: BoardHandle;
  r: FakeRenderer;
  sent: ClientMessage[];
  state: RoomState;
  c: ReturnType<typeof clock>;
  root: HTMLElement;
  over: ReturnType<typeof ctx2d>;
  under: ReturnType<typeof ctx2d>;
  gl: HTMLCanvasElement;
  snap: RoomSnapshot;
  receive(m: ServerMessage): void;
  /** Screen point of a piece's cell centre. */
  at(id: number): [number, number];
  pointer(type: string, id: number, x: number, y: number, o?: Partial<PointerEventInit>): void;
  drags(): ClientMessage[];
  setSocketState(st: 'open' | 'reconnecting'): void;
}

const DRAG_TYPES = new Set(['drag_start', 'drag_start_selection', 'move_piece', 'move_selection', 'drag_end', 'drag_end_selection', 'rotate_selection']);

let mounted: BoardHandle | null = null;

function setup(o: { snap?: Partial<RoomSnapshot>; spectator?: boolean; welcomeOver?: Record<string, unknown> } = {}): Setup {
  const c = clock();
  const sent: ClientMessage[] = [];
  const state = new RoomState({ now: () => c.now });
  const snap = baseSnapshot(V, { pieces: spreadPieces(), media_sha256: BUILTIN_MEDIA[0]!.sha256, media_extension: 'mp4', ...o.snap });
  state.applyServer(welcome(V, { snapshot: snap, spectator: o.spectator ?? false, ...o.welcomeOver }));
  const root = document.createElement('div');
  document.body.append(root);
  let renderer: FakeRenderer | null = null;
  const under = ctx2d();
  const over = ctx2d();
  const stateCbs: ((s: 'open') => void)[] = [];
  const h = mountBoard(root, {
    socket: {
      send: (m) => (sent.push(m), true),
      onState: (cb) => {
        stateCbs.push(cb as (s: 'open') => void);
        return () => void stateCbs.splice(stateCbs.indexOf(cb as (s: 'open') => void), 1);
      },
    },
    state,
    media: { assetBase: '/play-app/' },
    camera: new Camera(800, 600),
    createRenderer: (_canvas, _media, camera) => (renderer = new FakeRenderer(camera)),
    now: () => c.now,
    timers: c.timers,
    raf: c.raf,
    caf: c.caf,
    matchMedia: null,
    context2d: (cv) => (cv.classList.contains('po-board-under') ? under.ctx : over.ctx),
  });
  mounted = h;
  const gl = root.querySelector<HTMLCanvasElement>('canvas.po-board-gl')!;
  const cw = snap.width / snap.columns;
  const ch = snap.height / snap.rows;
  const s: Setup = {
    h, r: renderer!, sent, state, c, root, over, under, gl, snap,
    receive: (m) => state.applyServer(m),
    at(id) {
      const p = state.pieces().find((q) => q.id === id)!;
      return h.camera.toScreen(p.x + cw / 2, p.y + ch / 2);
    },
    pointer(type, id, x, y, extra = {}) {
      gl.dispatchEvent(new PointerEvent(type, {
        bubbles: true, cancelable: true, pointerId: id, pointerType: 'mouse', clientX: x, clientY: y,
        buttons: type === 'pointerup' ? 0 : 1, ...extra,
      }));
    },
    drags: () => sent.filter((m) => DRAG_TYPES.has(m.type)),
    setSocketState: (st) => {
      for (const cb of [...stateCbs]) cb(st as 'open');
    },
  };
  return s;
}

beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockImplementation(() => Promise.resolve());
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined);
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => undefined);
});

afterEach(() => {
  mounted?.destroy();
  mounted = null;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

// ---- tests --------------------------------------------------------------------

describe('board', () => {
  it('drag_sends_start_moves_end_in_order', () => {
    const s = setup();
    const [x, y] = s.at(1);
    const scale = s.h.camera.scale;
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    s.pointer('pointermove', 1, x + 40, y + 10);
    s.pointer('pointerup', 1, x + 40, y + 10);
    const d = s.drags();
    expect(d.map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'move_piece', 'drag_end']);
    expect(d[0]).toEqual({ type: 'drag_start', piece_id: 1 });
    const end = d[3] as Extract<ClientMessage, { type: 'drag_end' }>;
    expect(end.piece_id).toBe(1);
    expect(end.x).toBeCloseTo(1000 + 40 / scale, 3);
    expect(end.y).toBeCloseTo(100 + 10 / scale, 3);
    // The local piece followed the pointer and is drawn held.
    const last = s.r.last(1);
    expect(last[1]).toBeCloseTo(end.x, 3);
    expect(last[4] & FLAG_HELD).toBe(FLAG_HELD);
  });

  it('server_refusal_reverts_local_piece', () => {
    const s = setup();
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 50, y);
    expect(s.r.last(0)[1]).toBeGreaterThan(600);
    s.receive({ type: 'error', message: "That piece is in Bo's holder." });
    const back = s.r.last(0);
    expect([back[1], back[2]]).toEqual([600, 100]);
    expect(back[4] & FLAG_HELD).toBe(0);
    // No more moves for a refused (paused) drag, but the release still drops:
    // errors carry no id, so the client cannot know it was this drag's refusal.
    const before = s.drags().length;
    s.pointer('pointermove', 1, x + 80, y);
    expect(s.drags().length).toBe(before);
    s.pointer('pointerup', 1, x + 80, y);
    const after = s.drags().slice(before);
    expect(after.map((m) => m.type)).toEqual(['drag_end']);
    // The refusal is shown verbatim; the server's answer to the drop is not toasted over it.
    s.receive({ type: 'error', message: 'You do not hold that piece group' });
    const toast = s.root.querySelector('.po-board-toast')!;
    expect(toast.textContent).toBe("That piece is in Bo's holder.");
  });

  it('a_stray_error_before_the_grant_does_not_leak_the_lease', () => {
    const s = setup();
    const [x, y] = s.at(1);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    s.receive({ type: 'error', message: 'You are sending chat messages too quickly. Wait a moment.' });
    s.pointer('pointermove', 1, x + 30, y);
    // The grant arrives after all: back to a normal drag.
    s.receive({ type: 'pieces_changed', revision: 2, pieces: [{ ...s.snap.pieces[1], held_by: 'me' }] });
    expect(s.state.localDrag()?.anchorId).toBe(1);
    s.pointer('pointermove', 1, x + 40, y);
    s.pointer('pointerup', 1, x + 40, y);
    const d = s.drags();
    expect(d[d.length - 1].type).toBe('drag_end');
    const end = d[d.length - 1] as Extract<ClientMessage, { type: 'drag_end' }>;
    expect(end.x).toBeCloseTo(1000 + 40 / s.h.camera.scale, 3);
    expect(s.r.last(1)[1]).toBeCloseTo(end.x, 3);
  });

  it('a_refused_pickup_sends_exactly_one_drop_and_one_toast', () => {
    const s = setup();
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    s.receive({ type: 'error', message: "That piece is in Bo's holder." });
    for (let k = 0; k < 5; k++) s.pointer('pointermove', 1, x + 30 + k, y);
    s.pointer('pointerup', 1, x + 40, y);
    s.receive({ type: 'error', message: 'You do not hold that piece group' });
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
    expect(s.root.querySelector('.po-board-toast')!.textContent).toBe("That piece is in Bo's holder.");
    // A real "do not hold" later (nothing expected any more) is shown.
    s.receive({ type: 'error', message: 'You do not hold that piece group' });
    expect(s.root.querySelector('.po-board-toast')!.textContent).toBe('You do not hold that piece group');
  });

  it('a_drag_right_after_a_refused_flick_survives_the_trailing_error', () => {
    const s = setup();
    const [x0, y0] = s.at(0);
    s.pointer('pointerdown', 1, x0, y0);
    s.pointer('pointermove', 1, x0 + 20, y0);
    s.receive({ type: 'error', message: "That piece is in Bo's holder." });
    s.pointer('pointerup', 1, x0 + 20, y0);
    // Next drag starts before the drop's reply comes back.
    const [x, y] = s.at(1);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    s.receive({ type: 'error', message: 'You do not hold that piece group' });
    expect(s.state.localDrag()?.anchorId).toBe(1);
    s.pointer('pointermove', 1, x + 40, y);
    s.receive({ type: 'pieces_changed', revision: 2, pieces: [{ ...s.snap.pieces[1], held_by: 'me' }] });
    s.pointer('pointerup', 1, x + 40, y);
    const second = s.drags().slice(3);
    expect(second.map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'move_piece', 'drag_end']);
    expect(s.root.querySelector('.po-board-toast')!.textContent).toBe("That piece is in Bo's holder.");
  });

  it('group_drag_sends_selection_variants', () => {
    const s = setup({ snap: { pieces: spreadPieces().map((p) => (p.id === 2 ? { ...p, group_id: 1 } : p)) } });
    // ctrl-click pieces 0 and 1 (1's group includes 2).
    for (const id of [0, 1]) {
      const [x, y] = s.at(id);
      s.pointer('pointerdown', 1, x, y, { ctrlKey: true });
      s.pointer('pointerup', 1, x, y, { ctrlKey: true });
    }
    expect([...s.h.selection].sort()).toEqual([0, 1]);
    expect(s.r.last(0)[4] & FLAG_SELECTED).toBe(FLAG_SELECTED);
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 30, y);
    s.pointer('pointerup', 1, x + 30, y);
    const d = s.drags();
    expect(d.map((m) => m.type)).toEqual(['drag_start_selection', 'move_selection', 'drag_end_selection']);
    expect(d[0]).toEqual({ type: 'drag_start_selection', piece_ids: [0, 1] });
    expect((d[2] as { anchor_piece_id: number }).anchor_piece_id).toBe(0);
    // The local prediction moves the whole groups: 0, 1 and 2 (joined to 1), not 3.
    const dx = 30 / s.h.camera.scale;
    expect(s.r.last(2)[1]).toBeCloseTo(1400 + dx, 3);
    expect(s.r.last(3)[1]).toBe(1800);
  });

  it('clicking_an_unselected_piece_drags_it_alone_and_clears_the_selection', () => {
    const s = setup();
    const [x0, y0] = s.at(0);
    s.pointer('pointerdown', 1, x0, y0, { ctrlKey: true });
    s.pointer('pointerup', 1, x0, y0, { ctrlKey: true });
    const [x, y] = s.at(3);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 30, y);
    s.pointer('pointerup', 1, x + 30, y);
    expect(s.drags()[0]).toEqual({ type: 'drag_start', piece_id: 3 });
    expect(s.h.selection.size).toBe(0);
  });

  it('second_finger_cancel_drops_the_piece_back_with_drag_end_at_its_origin', () => {
    const s = setup();
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y, { pointerType: 'touch' });
    s.pointer('pointermove', 1, x + 40, y, { pointerType: 'touch' });
    s.pointer('pointerdown', 2, x + 200, y, { pointerType: 'touch' });
    const d = s.drags();
    expect(d.map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
    // The lease is released by a drop where the piece started.
    expect(d[2]).toEqual({ type: 'drag_end', piece_id: 0, x: 600, y: 100 });
    expect(s.state.localDrag()?.ended).toBe(true);
    expect([s.r.last(0)[1], s.r.last(0)[2]]).toEqual([600, 100]);
  });

  it('window_blur_mid_drag_drops_the_piece_back_once', () => {
    const s = setup();
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 40, y);
    window.dispatchEvent(new FocusEvent('blur'));
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
    expect(s.drags()[2]).toEqual({ type: 'drag_end', piece_id: 0, x: 600, y: 100 });
    expect(s.state.localDrag()?.ended ?? true).toBe(true);
    // The button coming up later (or more moves) sends nothing more.
    s.pointer('pointermove', 1, x + 80, y);
    s.pointer('pointerup', 1, x + 80, y);
    window.dispatchEvent(new FocusEvent('blur'));
    expect(s.drags()).toHaveLength(3);
  });

  it('page_hidden_mid_drag_drops_the_piece_back_once', () => {
    const s = setup();
    const vis = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y, { pointerType: 'touch' });
    s.pointer('pointermove', 1, x + 40, y, { pointerType: 'touch' });
    document.dispatchEvent(new Event('visibilitychange'));
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
    expect(s.drags()[2]).toEqual({ type: 'drag_end', piece_id: 0, x: 600, y: 100 });
    expect(s.state.localDrag()?.ended ?? true).toBe(true);
    s.pointer('pointerup', 1, x + 40, y, { pointerType: 'touch' });
    expect(s.drags()).toHaveLength(3);
    vis.mockRestore();
  });

  it('page_shown_again_or_blur_without_a_drag_sends_nothing', () => {
    const s = setup();
    window.dispatchEvent(new FocusEvent('blur'));
    document.dispatchEvent(new Event('visibilitychange'));
    expect(s.drags()).toEqual([]);
    // visible: a live drag goes on
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 40, y);
    document.dispatchEvent(new Event('visibilitychange'));
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece']);
    s.pointer('pointerup', 1, x + 40, y);
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
  });

  it('an_input_losing_focus_mid_drag_does_not_end_it', () => {
    const s = setup();
    const input = document.createElement('input');
    s.root.append(input);
    input.focus();
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 40, y);
    input.blur();
    // A blur event aimed at an element, even one that reaches window listeners.
    input.dispatchEvent(new FocusEvent('blur', { bubbles: true }));
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece']);
    expect(s.state.localDrag()).not.toBeNull();
  });

  it('destroy_removes_the_blur_and_visibility_listeners', () => {
    const removeWin = vi.spyOn(window, 'removeEventListener');
    const removeDoc = vi.spyOn(document, 'removeEventListener');
    const s = setup();
    s.h.destroy();
    mounted = null;
    expect(removeWin.mock.calls.filter(([t]) => t === 'blur').length).toBeGreaterThanOrEqual(1);
    expect(removeDoc.mock.calls.some(([t]) => t === 'visibilitychange')).toBe(true);
    // After destroy, nothing reacts.
    const before = s.sent.length;
    window.dispatchEvent(new FocusEvent('blur'));
    document.dispatchEvent(new Event('visibilitychange'));
    expect(s.sent.length).toBe(before);
  });

  it('a_held_piece_is_always_dropped_unless_the_pickup_was_refused', () => {
    const s = setup();
    const [x, y] = s.at(1);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    // The server confirms the lease; then the local overlay goes away for some
    // other reason. The lease must still be released by a drop.
    s.receive({ type: 'pieces_changed', revision: 2, pieces: [{ ...s.snap.pieces[1], held_by: 'me' }] });
    s.receive({ type: 'error', message: 'You are sending chat messages too quickly. Wait a moment.' });
    s.state.cancelDrag();
    s.pointer('pointermove', 1, x + 40, y);
    s.pointer('pointerup', 1, x + 40, y);
    const d = s.drags();
    expect(d[d.length - 1].type).toBe('drag_end');
  });

  it('a_dropped_connection_ends_the_drag_without_sending', () => {
    const s = setup();
    const [x, y] = s.at(1);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    s.state.connectionLost();
    s.setSocketState('reconnecting');
    s.pointer('pointermove', 1, x + 40, y);
    s.pointer('pointerup', 1, x + 40, y);
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece']);
    // Nothing stuck: after the resume the next drag is a normal one.
    s.receive(welcome(V, { snapshot: { ...s.snap, revision: 5 } }));
    s.setSocketState('open');
    expect(s.state.localDrag()).toBeNull();
    const [x2, y2] = s.at(2);
    s.pointer('pointerdown', 1, x2, y2);
    s.pointer('pointermove', 1, x2 + 20, y2);
    s.pointer('pointerup', 1, x2 + 20, y2);
    expect(s.drags().slice(2).map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
  });

  it('a_reconnect_welcome_mid_drag_ends_it_without_a_drop', () => {
    const s = setup();
    const [x, y] = s.at(1);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 20, y);
    s.receive(welcome(V, { snapshot: { ...s.snap, revision: 5 } }));
    s.pointer('pointermove', 1, x + 40, y);
    s.pointer('pointerup', 1, x + 40, y);
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece']);
    expect(s.state.localDrag()).toBeNull();
  });

  it('renderer_context_loss_keeps_state_and_input_alive', () => {
    const s = setup();
    const lostNote = s.root.querySelector<HTMLElement>('.po-board-lost')!;
    expect(lostNote.hidden).toBe(true);
    s.r.lose(true);
    expect(lostNote.hidden).toBe(false);
    // Input still drives the room while lost.
    const [x, y] = s.at(2);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 30, y);
    s.pointer('pointerup', 1, x + 30, y);
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
    // Server state keeps flowing into the renderer's CPU copy.
    s.receive({ type: 'pieces_changed', revision: 5, pieces: [{ ...s.snap.pieces[3], x: 42, y: 43 }] });
    expect(s.r.last(3).slice(1, 3)).toEqual([42, 43]);
    s.r.lose(false);
    expect(lostNote.hidden).toBe(true);
    expect(s.state.piece(3)?.x).toBe(42);
  });

  it('remote_cursor_expires_after_3s', () => {
    const s = setup();
    s.receive({ type: 'cursor', client_id: 'c2', name: 'Bo', x: 700, y: 150 });
    s.c.flushFrames();
    expect(s.over.texts()).toContain('Bo');
    s.over.calls.length = 0;
    s.c.advance(2999);
    expect(s.over.texts()).toContain('Bo');
    s.over.calls.length = 0;
    s.c.advance(2);
    s.c.flushFrames();
    expect(s.over.texts()).not.toContain('Bo');
    expect(s.state.cursors.size).toBe(0);
    // Nothing alive: the overlay stops asking for frames.
    s.c.flushFrames();
    expect(s.c.frames.size).toBe(0);
  });

  it('sends_cursor_on_hover_and_cursor_hidden_on_leave', () => {
    const s = setup();
    s.gl.dispatchEvent(new PointerEvent('pointermove', { pointerId: 1, pointerType: 'mouse', clientX: 300, clientY: 200 }));
    const [tx, ty] = s.h.camera.toTable(300, 200);
    expect(s.sent.filter((m) => m.type === 'cursor')).toEqual([{ type: 'cursor', x: tx, y: ty }]);
    s.gl.dispatchEvent(new PointerEvent('pointerleave', { pointerId: 1, pointerType: 'mouse' }));
    expect(s.sent[s.sent.length - 1]).toEqual({ type: 'cursor_hidden' });
  });

  it('ping_shows_3s', () => {
    const s = setup();
    // Double-tap on empty table sends a ping there.
    const tap = (id: number) => {
      s.pointer('pointerdown', id, 5, 5, { pointerType: 'touch' });
      s.pointer('pointerup', id, 5, 5, { pointerType: 'touch' });
    };
    tap(1);
    tap(2);
    const [tx, ty] = s.h.camera.toTable(5, 5);
    expect(s.sent.filter((m) => m.type === 'ping')).toEqual([{ type: 'ping', x: tx, y: ty }]);
    // The server relays it to everyone, the sender included.
    s.receive({ type: 'ping', client_id: 'me', name: 'Ann', x: tx, y: ty });
    s.over.calls.length = 0;
    s.c.flushFrames();
    expect(s.over.calls.some((c) => c[0] === 'arc')).toBe(true);
    s.c.advance(2999);
    s.over.calls.length = 0;
    s.c.flushFrames();
    expect(s.over.calls.some((c) => c[0] === 'arc')).toBe(true);
    s.c.advance(2);
    s.over.calls.length = 0;
    s.c.flushFrames();
    expect(s.over.calls.some((c) => c[0] === 'arc')).toBe(false);
  });

  it('draws_tray_area_and_holders_from_the_snapshot', () => {
    const s = setup({ snap: { holders: [{ id: 1, owner: 'Bo', label: 'edges', x: -400, y: 0, width: 200, height: 200 }] } });
    s.c.flushFrames();
    expect(s.under.texts()).toContain('edges · Bo');
    s.under.calls.length = 0;
    s.receive({ type: 'pieces_changed', revision: 9, pieces: [], holders: [] });
    s.c.flushFrames();
    expect(s.under.texts()).not.toContain('edges · Bo');
  });

  it('clock_ticks_locally_from_stats_and_the_server_clock', () => {
    const s = setup({
      welcomeOver: { server_now_ms: 1_000_000 },
      snap: { stats: { elapsed_ms: 60_000, running_since_ms: 1_000_000 - 5_000, started: true, finished: false, players: [] } },
    });
    const el = s.root.querySelector<HTMLElement>('.po-board-clock')!;
    expect(el.textContent).toBe('1:05');
    s.c.advance(1000);
    expect(el.textContent).toBe('1:06');
    // Stopped: the clock holds still.
    s.receive({ type: 'pieces_changed', revision: 9, pieces: [], stats: { elapsed_ms: 70_000, running_since_ms: null, started: true, finished: true, players: [] } });
    expect(el.textContent).toBe('1:10');
    s.c.advance(5000);
    expect(el.textContent).toBe('1:10');
    expect(formatClock(3_725_000)).toBe('1:02:05');
  });

  it('media_is_the_current_items_builtin_loop_with_poster', () => {
    const second = BUILTIN_MEDIA[1]!;
    const s = setup({
      snap: {
        queue: [
          { sha256: BUILTIN_MEDIA[0]!.sha256, extension: 'mp4', title: 'a', width: 480, height: 320, duration_ms: 8000, available: true, position_ms: 0, loop_start_ms: 0, loop_end_ms: 0 },
          { sha256: second.sha256, extension: 'mp4', title: 'b', width: 480, height: 320, duration_ms: 8000, available: true, position_ms: 0, loop_start_ms: 0, loop_end_ms: 0 },
        ],
        current: 1,
      },
    });
    const video = s.root.querySelector('video')!;
    const poster = s.root.querySelector<HTMLImageElement>('img.po-board-poster')!;
    expect(video.muted).toBe(true);
    expect(video.loop).toBe(true);
    expect(video.hasAttribute('playsinline')).toBe(true);
    expect(video.getAttribute('src')).toMatch(new RegExp(`^/play-app/builtin/${second.sha256}\\.mp4$`));
    expect(poster.getAttribute('src')).toMatch(new RegExp(`^/play-app/builtin/${second.sha256}\\.jpg$`));
  });

  it('media_resyncs_when_the_video_drifts', () => {
    const s = setup({ welcomeOver: { server_now_ms: 10_000 }, snap: { playback: { playing: true, position_ms: 0, rate: 1, loop_start_ms: 0, loop_end_ms: 0, changed_at_ms: 10_000 } } });
    const video = s.root.querySelector('video')!;
    Object.defineProperty(video, 'readyState', { value: 4, configurable: true });
    Object.defineProperty(video, 'duration', { value: 8, configurable: true });
    Object.defineProperty(video, 'seeking', { value: false, configurable: true });
    let t = 0;
    Object.defineProperty(video, 'currentTime', { get: () => t, set: (v: number) => void (t = v), configurable: true });
    s.c.advance(2000); // room at 2 s, video stuck at 0
    expect(video.currentTime).toBeCloseTo(2, 1);
  });

  it('select_toggle_makes_touch_taps_select', () => {
    const changes: boolean[] = [];
    const s = setup();
    s.h.onSelectModeChange((on) => changes.push(on));
    const btn = s.root.querySelector<HTMLButtonElement>('.po-board-select')!;
    btn.click();
    expect(btn.getAttribute('aria-pressed')).toBe('true');
    expect(changes).toEqual([true]);
    const [x, y] = s.at(2);
    s.pointer('pointerdown', 1, x, y, { pointerType: 'touch' });
    s.pointer('pointerup', 1, x, y, { pointerType: 'touch' });
    expect([...s.h.selection]).toEqual([2]);
    s.h.setSelectMode(false);
    expect(btn.getAttribute('aria-pressed')).toBe('false');
    expect(changes).toEqual([true, false]);
  });

  it('spectators_neither_drag_nor_ping', () => {
    const s = setup({ spectator: true });
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 30, y);
    s.pointer('pointerup', 1, x + 30, y);
    expect(s.drags()).toEqual([]);
    expect(s.sent.filter((m) => m.type === 'cursor')).toEqual([]);
  });

  it('turn_buttons_are_hidden_without_rotation', () => {
    const s = setup();
    const turns = [...s.root.querySelectorAll<HTMLButtonElement>('.po-board-turn')];
    expect(turns).toHaveLength(2);
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 30, y);
    expect(turns.every((b) => b.hidden)).toBe(true); // this room does not turn its pieces
  });

  it.skipIf(!HAS_ROTATION)('rotation_button_sends_rotate_selection', () => {
    const s = setup({ snap: { difficulty: { rotation: true, no_hints: false } } as unknown as Partial<RoomSnapshot> });
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y, { pointerType: 'touch' });
    s.pointer('pointermove', 1, x + 30, y, { pointerType: 'touch' });
    const right = s.root.querySelector<HTMLButtonElement>('.po-board-turn[data-turn="1"]')!;
    expect(right.hidden).toBe(false);
    right.click();
    const rot = s.drags().filter((m) => (m.type as string) === 'rotate_selection');
    expect(rot).toEqual([{ type: 'rotate_selection', anchor_piece_id: 0, quarter_turns: 1 }]);
  });

  it('destroy_cleans_everything', () => {
    const add = vi.spyOn(window, 'addEventListener');
    const remove = vi.spyOn(window, 'removeEventListener');
    const observers: { disconnected: boolean }[] = [];
    vi.stubGlobal('ResizeObserver', class {
      disconnected = false;
      constructor(_cb: ResizeObserverCallback) {
        observers.push(this);
      }
      observe() {}
      disconnect() {
        this.disconnected = true;
      }
    });
    const s = setup({ snap: { stats: { elapsed_ms: 0, running_since_ms: 1, started: true, finished: false, players: [] } } });
    // A live drag, a cursor and a ping in flight.
    const [x, y] = s.at(0);
    s.pointer('pointerdown', 1, x, y);
    s.pointer('pointermove', 1, x + 30, y);
    s.receive({ type: 'cursor', client_id: 'c2', name: 'Bo', x: 700, y: 150 });
    expect(s.c.pending.size).toBeGreaterThan(0);
    s.h.destroy();
    mounted = null;
    // The held piece is let go (drag_end at its origin), the room hears we left the table.
    expect(s.drags().map((m) => m.type)).toEqual(['drag_start', 'move_piece', 'drag_end']);
    expect(s.sent[s.sent.length - 1]).toEqual({ type: 'cursor_hidden' });
    expect(s.r.disposed).toBe(true);
    expect(s.c.pending.size).toBe(0);
    expect(s.c.frames.size).toBe(0);
    expect(observers.length).toBeGreaterThan(0);
    expect(observers.every((o) => o.disconnected)).toBe(true);
    expect(s.root.children).toHaveLength(0);
    const count = (spy: typeof add) => spy.mock.calls.map((c) => c[0]).sort();
    expect(count(remove)).toEqual(expect.arrayContaining(count(add)));
    // No more reactions to the room.
    const n = s.r.calls.length;
    s.receive({ type: 'pieces_changed', revision: 9, pieces: [{ ...s.snap.pieces[1], x: 1, y: 1 }] });
    s.receive({ type: 'ping', client_id: 'x', name: 'Bo', x: 1, y: 1 });
    expect(s.r.calls.length).toBe(n);
    expect(s.c.frames.size).toBe(0);
    vi.unstubAllGlobals();
  });
});
