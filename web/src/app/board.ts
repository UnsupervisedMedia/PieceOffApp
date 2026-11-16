// The board screen (lazy chunk): WebGL pieces, Canvas 2D table/overlays, the
// gesture machine, and the room's messages, wired the way the desktop board is
// (crates/pieceoff-app/src/board.rs, cursors.rs; ARCHITECTURE.md "Multiplayer
// model"). The server is the authority: the client shows its own drag at once
// (RoomState's optimistic overlay) and otherwise draws what `welcome`,
// `snapshot` and `pieces_changed` say.
//
// Decisions (see the Task 9 report):
// - Drags: a piece that is not part of a multi-selection uses drag_start /
//   move_piece / drag_end (the server moves its whole group); dragging a piece
//   of a selection of two or more uses the _selection variants with the
//   selected ids, and the local prediction moves every loose piece of the
//   selected groups, like the desktop. Moves go through RoomSocket, which caps
//   them at 30/s and lets the drop carry the final spot.
// - Cancel (second finger, pointercancel, destroy): the protocol has no
//   drag-abort, and a lease must not stay held, so the client drops the piece
//   where it started: drag_end(_selection) at the anchor's pre-drag position.
//   Locally the piece goes back there at once and waits for the drop.
// - A refused pick-up arrives as an `error` before the server has confirmed the
//   lease (RoomState drops the unconfirmed overlay, so the piece snaps back);
//   the rest of that gesture sends nothing. A dropped connection (or a
//   reconnect's welcome) ends it the same way: the server already expired the
//   lease. In every other case the release always sends the drop, even if the
//   local overlay went away meanwhile, so a lease is never left held.
//   Server errors show verbatim in a short toast.
// - Placed pieces and pieces someone else holds are not grabbable: pressing
//   them pans (touch) or rubber-bands (mouse) instead of asking for a refusal.
// - Cursors: own pointer sent as `cursor` while over the table (RoomSocket
//   throttles to 15/s and drops repeats), `cursor_hidden` on leaving the canvas
//   or the window; remote ones expire 3 s after their last update. Pings: a
//   double-tap sends `ping`; the server echoes it to everyone, and each shows 3 s.
// - Solve clock ticks locally from `stats` against the server clock offset.
// - Media: the current queue item's built-in loop as a muted, looping,
//   inline <video> (poster first and on any failure), kept on the room clock by
//   MediaSync (seek when more than 300 ms off).
import { board_layout, piece_outlines } from '../../wasm/pieceoff_web_wasm.js';
import { Camera } from '../gl/camera';
import { defaultMatchMedia, watchDpr, type MatchMedia } from '../gl/dpr';
import { LodPolicy } from '../gl/lod';
import { buildMesh, type Mesh, type OutlineSource } from '../gl/mesh';
import { OverlayLayers, OverlayModel, type BoardLayout } from '../gl/overlays';
import { BoardRenderer, type BoardMedia } from '../gl/renderer';
import { FLAG_HELD, FLAG_PLACED, FLAG_SELECTED } from '../gl/shaders';
import { GestureMachine, type GestureOptions, type Rect } from '../input/gestures';
import { hitTest, type PieceGeom } from '../input/hit';
import { attachPointerEvents } from '../input/pointer';
import { builtinAssets, type BuiltinAssets } from '../media/builtin';
import { MediaSync } from '../media/sync';
import type { RoomState } from '../net/roomState';
import type { FatalReason, SocketState } from '../net/socket';
import type { ClientMessage, RoomSnapshot, ServerMessage, SolveStats } from '../protocol';
import { button, h } from '../ui/dom';
import type { BoardModule } from './loadBoard';
import { HAS_ROTATION, pieceRotation, roomRotates, rotateMessage } from './rotation';

/** How long a server error stays on screen. */
export const TOAST_MS = 4000;

/** What the board needs of RoomSocket. */
export interface BoardSocket {
  send(m: ClientMessage): boolean;
  onState(cb: (s: SocketState, why?: FatalReason) => void): () => void;
}

/** What the board needs of BoardRenderer (tests inject a fake). */
export interface RendererLike {
  readonly camera: Camera;
  readonly contextLost: boolean;
  setPuzzle(mesh: Mesh, snapshot: RoomSnapshot): void;
  setPiece(id: number, x: number, y: number, rotation: number, flags: number): void;
  pieceDepth(id: number): number;
  requestRender(): void;
  onContextLost(cb: (lost: boolean) => void): void;
  resize(): void;
  dispose(): void;
}

export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface BoardDeps {
  /** The joined room's socket (drag/cursor throttling lives there). */
  socket: BoardSocket;
  /** Fed by the socket (`RoomState.attach`); the board only reads it and drives the local drag. */
  state: RoomState;
  /** Where the built-in loops are served: `${assetBase}builtin/`. `assets` overrides the lookup. */
  media: { assetBase: string; assets?: (sha256: string) => BuiltinAssets | null };
  /** The camera shared by renderer, overlays and input; one is made if omitted. */
  camera?: Camera;
  /** Watching: no drags, cursors or pings are sent (default: the welcome's). */
  spectator?: boolean;
  /** Called with the select toggle's new state (the toolbar mirrors it). */
  onSelectModeChange?: (on: boolean) => void;
  /** A "Leave room" button is shown when given. */
  leave?: () => void;
  // ---- seams (tests) ----
  createRenderer?: (canvas: HTMLCanvasElement, media: BoardMedia, camera: Camera) => RendererLike;
  outlines?: (snapshot: RoomSnapshot) => OutlineSource;
  layout?: (snapshot: RoomSnapshot) => BoardLayout;
  now?: () => number;
  timers?: Timers;
  raf?: (cb: FrameRequestCallback) => number;
  caf?: (h: number) => void;
  matchMedia?: MatchMedia | null;
  gestures?: Partial<GestureOptions>;
  context2d?: (c: HTMLCanvasElement) => CanvasRenderingContext2D | null;
}

export interface BoardHandle {
  destroy(): void;
  readonly camera: Camera;
  /** Selected piece ids (read-only view). */
  readonly selection: ReadonlySet<number>;
  setSelectMode(on: boolean): void;
  /** Subscribe to select-mode changes (toolbar). Returns the unsubscribe. */
  onSelectModeChange(cb: (on: boolean) => void): () => void;
}

// ---- solve clock -------------------------------------------------------------

/** Solve time at server time `serverNow`: banked time plus the running stretch. */
export function solveElapsedMs(stats: SolveStats, serverNow: number): number {
  const run = stats.running_since_ms === null ? 0 : Math.max(0, serverNow - stats.running_since_ms);
  return stats.elapsed_ms + run;
}

/** m:ss, or h:mm:ss from an hour. */
export function formatClock(ms: number): string {
  const t = Math.max(0, Math.floor(ms / 1000));
  const s = t % 60;
  const m = Math.floor(t / 60) % 60;
  const hr = Math.floor(t / 3600);
  const ss = String(s).padStart(2, '0');
  return hr > 0 ? `${hr}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

// ---- geometry ----------------------------------------------------------------

function parseLayout(snapshot: RoomSnapshot): BoardLayout {
  return JSON.parse(board_layout(JSON.stringify(snapshot))) as BoardLayout;
}

function wasmOutlines(snapshot: RoomSnapshot): OutlineSource {
  const out = piece_outlines(JSON.stringify(snapshot));
  const copy = { ids: out.ids, offsets: out.offsets, points: out.points };
  out.free();
  return copy;
}

/** What identifies one puzzle's shapes: a change means new outlines and mesh. */
function puzzleKey(s: RoomSnapshot): string {
  return `${s.media_sha256}|${s.width}x${s.height}|${s.rows}x${s.columns}|${s.cut_style}|${s.shape_seed}|${s.pieces.length}`;
}

function currentSha(s: RoomSnapshot): string {
  return s.queue[s.current]?.sha256 ?? s.media_sha256;
}

// ---- the board -----------------------------------------------------------------

export function mountBoard(root: HTMLElement, deps: BoardDeps): BoardHandle {
  const { socket, state } = deps;
  const now = deps.now ?? (() => Date.now());
  const timers: Timers = deps.timers ?? {
    set: (fn, ms) => setTimeout(fn, ms),
    clear: (hd) => clearTimeout(hd as ReturnType<typeof setTimeout>),
  };
  const raf = deps.raf ?? ((cb: FrameRequestCallback) => requestAnimationFrame(cb));
  const caf = deps.caf ?? ((hd: number) => cancelAnimationFrame(hd));
  const mm = deps.matchMedia === undefined ? defaultMatchMedia() : deps.matchMedia;
  const spectator = () => deps.spectator ?? state.spectator;
  const cleanups: (() => void)[] = [];
  const listen = (t: EventTarget, type: string, fn: EventListener, o?: AddEventListenerOptions) => {
    t.addEventListener(type, fn, o);
    cleanups.push(() => t.removeEventListener(type, fn, o));
  };

  // ---- DOM ----
  const under = h('canvas', { class: 'po-board-under', 'aria-hidden': 'true' }) as HTMLCanvasElement;
  const glCanvas = h('canvas', { class: 'po-board-gl', 'aria-label': 'Puzzle board' }) as HTMLCanvasElement;
  const over = h('canvas', { class: 'po-board-over', 'aria-hidden': 'true' }) as HTMLCanvasElement;
  const poster = h('img', { class: 'po-board-poster', alt: '' }) as HTMLImageElement;
  poster.hidden = true;
  const video = h('video', { class: 'po-board-video', 'aria-hidden': 'true' }) as HTMLVideoElement;
  video.muted = true;
  video.defaultMuted = true;
  video.loop = true;
  video.playsInline = true;
  video.setAttribute('playsinline', '');
  video.setAttribute('muted', '');
  video.preload = 'auto';
  video.crossOrigin = 'anonymous';
  const lostNote = h('div', { class: 'po-board-lost', role: 'status' }, 'Graphics paused. The board comes back when the browser restores it; you can keep playing.');
  lostNote.hidden = true;
  const stage = h('div', { class: 'po-board-stage' }, under, glCanvas, over, poster, video, lostNote);
  const clockEl = h('span', { class: 'po-board-clock', 'aria-label': 'Solve time' }, '0:00');
  const netEl = h('span', { class: 'po-board-net', role: 'status' });
  const selectBtn = button('Select', () => setSelectMode(!machine.selecting), { class: 'btn po-board-select', 'aria-pressed': 'false' });
  const turnLeft = button('⟲', () => turn(-1), { class: 'btn po-board-turn', 'data-turn': '-1', 'aria-label': 'Turn left' });
  const turnRight = button('⟳', () => turn(1), { class: 'btn po-board-turn', 'data-turn': '1', 'aria-label': 'Turn right' });
  turnLeft.hidden = true;
  turnRight.hidden = true;
  const code = state.snapshot()?.invite_code ?? '';
  const hud = h(
    'div',
    { class: 'po-board-hud' },
    h('code', { class: 'po-board-code' }, code),
    clockEl,
    netEl,
    h('span', { class: 'po-board-spacer' }),
    selectBtn,
    turnLeft,
    turnRight,
    deps.leave ? button('Leave', () => deps.leave!(), { class: 'btn po-board-leave' }) : null,
  );
  const toast = h('div', { class: 'po-board-toast', role: 'status', 'aria-live': 'polite' });
  toast.hidden = true;
  const section = h('section', { class: 'po-board', 'aria-label': 'Puzzle' }, stage, hud, toast);
  root.replaceChildren(section);

  // ---- renderer, overlays ----
  const camera = deps.camera ?? new Camera(1, 1);
  const media: BoardMedia = { video, poster };
  const renderer: RendererLike = deps.createRenderer
    ? deps.createRenderer(glCanvas, media, camera)
    : new BoardRenderer(glCanvas, media, { camera, transparent: true, raf, caf, matchMedia: mm });
  const model = new OverlayModel();
  const overlays = new OverlayLayers(under, over, camera, {
    model,
    cursors: () => state.cursors,
    now,
    raf,
    caf,
    context2d: deps.context2d,
  });
  renderer.onContextLost((lost) => {
    lostNote.hidden = !lost;
  });

  // ---- puzzle geometry ----
  let key = '';
  let snap: RoomSnapshot | null = null;
  let outline: OutlineSource | null = null;
  let layout: BoardLayout | null = null;
  let geoms: PieceGeom[] = [];
  let cw = 1;
  let ch = 1;
  const indexOf = new Map<number, number>();
  let px = new Float32Array(0);
  let py = new Float32Array(0);
  let pr = new Uint8Array(0);
  let pf = new Int16Array(0);
  let lod: LodPolicy | null = null;
  let fitted = false;
  let userMoved = false;
  let holdersRef: unknown = null;
  const selection = new Set<number>();

  const fit = () => {
    if (!snap) return;
    let minX = 0, minY = 0, maxX = snap.width, maxY = snap.height;
    if (layout) {
      const t = layout.tray;
      const p = layout.tray_pad;
      minX = Math.min(minX, t.x - p);
      minY = Math.min(minY, t.y - p);
      maxX = Math.max(maxX, t.x + t.width + p);
      maxY = Math.max(maxY, t.y + t.height + p);
    }
    for (const p of snap.pieces) {
      minX = Math.min(minX, p.x);
      minY = Math.min(minY, p.y);
      maxX = Math.max(maxX, p.x + cw);
      maxY = Math.max(maxY, p.y + ch);
    }
    const m = Math.max(cw, ch) * 0.5;
    camera.fit(maxX - minX + 2 * m, maxY - minY + 2 * m, minX - m, minY - m);
    viewChanged();
  };

  const build = (maxSamples: number) => {
    if (!outline || !snap) return;
    const mesh = buildMesh(outline, snap, { maxSamples });
    renderer.setPuzzle(mesh, snap);
    lod?.built(mesh.strides[0] ?? 1);
  };

  const loadPuzzle = (s: RoomSnapshot) => {
    key = puzzleKey(s);
    snap = s;
    outline = (deps.outlines ?? wasmOutlines)(s);
    layout = (deps.layout ?? parseLayout)(s);
    cw = s.width / s.columns;
    ch = s.height / s.rows;
    const n = outline.ids.length;
    indexOf.clear();
    outline.ids.forEach((id, i) => indexOf.set(id, i));
    px = new Float32Array(n).fill(Number.NaN);
    py = new Float32Array(n).fill(Number.NaN);
    pr = new Uint8Array(n);
    pf = new Int16Array(n).fill(-1);
    const pts = outline.points;
    const offs = outline.offsets;
    geoms = Array.from(outline.ids, (id, i) => ({
      id, x: 0, y: 0, rotation: 0, pivotX: cw / 2, pivotY: ch / 2, depth: 0,
      points: pts.subarray(offs[i], offs[i + 1]),
    }));
    lod?.dispose();
    const samples0 = n > 0 ? (offs[1] - offs[0]) / 2 : 1;
    lod = new LodPolicy({ samples0, cellW: cw, cellH: ch, timers, rebuild: (max) => build(max) });
    selection.clear();
    overlays.setScene({ layout, boardW: s.width, boardH: s.height });
    if (!fitted) {
      fitted = true;
      fit();
    }
    build(lod.samplesAt(camera.scale));
    loadMedia(s);
  };

  /** Push every piece whose drawn state changed. */
  const pushPieces = () => {
    if (!snap) return;
    const drag = state.localDrag();
    const dragging = drag ? new Set(drag.pieceIds) : null;
    for (const p of state.pieces()) {
      const i = indexOf.get(p.id);
      if (i === undefined) continue;
      if ((p.placed || (p.held_by !== null && p.held_by !== state.clientId)) && selection.has(p.id)) selection.delete(p.id);
      let flags = 0;
      if (p.placed) flags |= FLAG_PLACED;
      if (p.held_by !== null || dragging?.has(p.id)) flags |= FLAG_HELD;
      if (selection.has(p.id)) flags |= FLAG_SELECTED;
      const rot = pieceRotation(p);
      if (px[i] !== p.x || py[i] !== p.y || pr[i] !== rot || pf[i] !== flags) {
        px[i] = p.x;
        py[i] = p.y;
        pr[i] = rot;
        pf[i] = flags;
        renderer.setPiece(p.id, p.x, p.y, rot, flags);
      }
    }
    updateTurnButtons();
  };

  // ---- media ----
  let mediaSha = '';
  const sync = new MediaSync(video, { serverNow: () => state.serverNow(), timers });
  const loadMedia = (s: RoomSnapshot) => {
    const sha = currentSha(s);
    if (sha === mediaSha) return;
    mediaSha = sha;
    const assets = deps.media.assets ? deps.media.assets(sha) : builtinAssets(sha, `${deps.media.assetBase}builtin/`);
    if (!assets) {
      video.removeAttribute('src');
      poster.removeAttribute('src');
      showToast('This picture is not a built-in loop; the web client cannot show it yet.');
      return;
    }
    poster.src = assets.poster;
    video.poster = assets.poster;
    video.src = assets.mp4;
    try {
      video.load();
    } catch {
      // jsdom / old engines
    }
    sync.apply(s.playback);
  };

  // ---- solve clock ----
  let clockTimer: unknown = null;
  const tickClock = () => {
    clockTimer = null;
    const s = state.snapshot();
    if (!s) return;
    const ms = solveElapsedMs(s.stats, state.serverNow());
    clockEl.textContent = formatClock(ms);
    if (s.stats.running_since_ms !== null) {
      // Wake at the next whole second of the clock.
      clockTimer = timers.set(tickClock, 1000 - (ms % 1000) || 1000);
    }
  };
  const restartClock = () => {
    if (clockTimer !== null) timers.clear(clockTimer);
    clockTimer = null;
    tickClock();
  };

  // ---- toast ----
  let toastTimer: unknown = null;
  const showToast = (text: string) => {
    toast.textContent = text;
    toast.hidden = false;
    if (toastTimer !== null) timers.clear(toastTimer);
    toastTimer = timers.set(() => {
      toastTimer = null;
      toast.hidden = true;
    }, TOAST_MS);
  };

  // ---- view ----
  const viewChanged = () => {
    renderer.requestRender();
    overlays.requestDraw();
    lod?.viewChanged(camera.scale);
  };
  const moved = () => {
    userMoved = true;
    viewChanged();
  };

  // ---- drag ----
  interface Drag {
    anchor: number;
    multi: boolean;
    dx: number;
    dy: number;
    ox: number;
    oy: number;
    x: number;
    y: number;
    /** The server said we hold the anchor. */
    confirmed: boolean;
    /** Refused, or the connection went: send nothing more for this drag. */
    dead: boolean;
  }
  let drag: Drag | null = null;

  const grabbable = (id: number): boolean => {
    const p = state.piece(id);
    return !!p && !p.placed && (p.held_by === null || p.held_by === state.clientId);
  };

  const hit = (sx: number, sy: number, tolPx: number): number | null => {
    if (spectator() || !snap) return null;
    const list: PieceGeom[] = [];
    for (const p of state.pieces()) {
      const i = indexOf.get(p.id);
      if (i === undefined || !grabbable(p.id)) continue;
      const g = geoms[i];
      g.x = p.x;
      g.y = p.y;
      g.rotation = pieceRotation(p);
      g.depth = renderer.pieceDepth(p.id);
      list.push(g);
    }
    const [tx, ty] = camera.toTable(sx, sy);
    return hitTest(list, tx, ty, tolPx / camera.scale);
  };

  const sendDrop = (d: Drag, x: number, y: number) => {
    socket.send(
      d.multi
        ? { type: 'drag_end_selection', anchor_piece_id: d.anchor, x, y }
        : { type: 'drag_end', piece_id: d.anchor, x, y },
    );
    state.endDrag();
  };

  const finishDrag = (cancelled: boolean) => {
    const d = drag;
    drag = null;
    if (!d || d.dead) return updateTurnButtons();
    if (cancelled) {
      // No drag-abort in the protocol: put it down where it started.
      if (state.localDrag()) state.dragTo(d.ox, d.oy);
      sendDrop(d, d.ox, d.oy);
    } else {
      sendDrop(d, d.x, d.y);
    }
    updateTurnButtons();
  };

  const turn = (turns: 1 | -1) => {
    const d = drag;
    if (!d || d.dead || !state.localDrag()) return;
    const m = rotateMessage(d.anchor, turns);
    if (m) socket.send(m);
  };

  const updateTurnButtons = () => {
    const show = HAS_ROTATION && roomRotates(state.snapshot()) && !!drag && !drag.dead && !!state.localDrag();
    turnLeft.hidden = !show;
    turnRight.hidden = !show;
  };

  const selectRect = (r: Rect, additive: boolean) => {
    if (!snap) return;
    if (!additive) selection.clear();
    for (const p of state.pieces()) {
      const x = p.x + cw / 2;
      const y = p.y + ch / 2;
      if (x >= r.x && y >= r.y && x <= r.x + r.w && y <= r.y + r.h && grabbable(p.id)) selection.add(p.id);
    }
    pushPieces();
  };

  const machine = new GestureMachine(
    {
      hit,
      toTable: (sx, sy) => camera.toTable(sx, sy),
      dragStart(id, tx, ty) {
        const p = state.piece(id);
        if (!p || spectator()) return;
        const multi = selection.has(id) && selection.size > 1;
        if (!selection.has(id)) selection.clear();
        let ids: number[] | undefined;
        let ok: boolean;
        if (multi) {
          const sel = [...selection].sort((a, b) => a - b);
          const groups = new Set(sel.map((i) => state.piece(i)?.group_id));
          ids = (state.snapshot()?.pieces ?? []).filter((q) => !q.placed && groups.has(q.group_id)).map((q) => q.id);
          ok = socket.send({ type: 'drag_start_selection', piece_ids: sel });
        } else {
          ok = socket.send({ type: 'drag_start', piece_id: id });
        }
        drag = { anchor: id, multi, dx: tx - p.x, dy: ty - p.y, ox: p.x, oy: p.y, x: p.x, y: p.y, confirmed: false, dead: !ok };
        if (ok) state.beginDrag(id, ids);
        updateTurnButtons();
      },
      dragMove(tx, ty) {
        const d = drag;
        if (!d || d.dead) return;
        d.x = tx - d.dx;
        d.y = ty - d.dy;
        if (state.localDrag()) state.dragTo(d.x, d.y);
        socket.send(
          d.multi
            ? { type: 'move_selection', anchor_piece_id: d.anchor, x: d.x, y: d.y }
            : { type: 'move_piece', piece_id: d.anchor, x: d.x, y: d.y },
        );
      },
      dragEnd: () => finishDrag(false),
      dragCancel: () => finishDrag(true),
      pan(dx, dy) {
        camera.panBy(dx, dy);
        moved();
      },
      pinch(cx, cy, f) {
        camera.zoomAt(cx, cy, f);
        moved();
      },
      wheelZoom(sx, sy, f) {
        camera.zoomAt(sx, sy, f);
        moved();
      },
      toggleSelect(id) {
        if (!grabbable(id)) return;
        if (selection.has(id)) selection.delete(id);
        else selection.add(id);
        pushPieces();
      },
      rubberBand: (r) => overlays.setRubber(r),
      selectRect,
      ping(tx, ty) {
        if (spectator() || !inScene(tx, ty)) return;
        socket.send({ type: 'ping', x: tx, y: ty });
      },
    },
    { timers, ...deps.gestures },
  );
  const detachPointer = attachPointerEvents(glCanvas, machine, camera);

  const selectCbs: ((on: boolean) => void)[] = [];
  if (deps.onSelectModeChange) selectCbs.push(deps.onSelectModeChange);
  const setSelectMode = (on: boolean) => {
    if (machine.selecting === on) return;
    machine.setSelectMode(on);
    selectBtn.setAttribute('aria-pressed', String(on));
    selectBtn.classList.toggle('is-on', on);
    for (const cb of [...selectCbs]) cb(on);
  };

  // ---- own cursor ----
  let cursorShown = false;
  const inScene = (x: number, y: number) => {
    const s = layout?.scene;
    return !!s && Number.isFinite(x) && Number.isFinite(y) && x >= s.min_x && x <= s.max_x && y >= s.min_y && y <= s.max_y;
  };
  const hideCursor = () => {
    if (!cursorShown) return;
    cursorShown = false;
    socket.send({ type: 'cursor_hidden' });
  };
  listen(glCanvas, 'pointermove', ((e: PointerEvent) => {
    if (spectator()) return;
    const r = glCanvas.getBoundingClientRect();
    const [tx, ty] = camera.toTable(e.clientX - r.left, e.clientY - r.top);
    if (inScene(tx, ty)) {
      cursorShown = true;
      socket.send({ type: 'cursor', x: tx, y: ty });
    } else {
      hideCursor();
    }
  }) as EventListener);
  listen(glCanvas, 'pointerleave', hideCursor);
  listen(window, 'blur', hideCursor);
  // A user gesture lets a refused autoplay start.
  listen(glCanvas, 'pointerdown', () => sync.nudge());
  listen(window, 'keydown', ((e: KeyboardEvent) => {
    if (e.key === 'Escape' && selection.size && !e.defaultPrevented) {
      selection.clear();
      pushPieces();
    }
  }) as EventListener);

  // ---- size ----
  const resize = () => {
    renderer.resize();
    overlays.resize(stage.clientWidth || camera.viewW, stage.clientHeight || camera.viewH);
    if (!userMoved && fitted) fit();
    viewChanged();
  };
  let observer: ResizeObserver | null = null;
  if (typeof ResizeObserver !== 'undefined') {
    observer = new ResizeObserver(() => resize());
    observer.observe(stage);
  }
  cleanups.push(watchDpr(mm, resize));

  // ---- the room ----
  const onMessage = (m: ServerMessage | null) => {
    const s = state.snapshot();
    if (!s) return;
    if (s !== snap) {
      if (puzzleKey(s) !== key) loadPuzzle(s);
      else {
        snap = s;
        loadMedia(s);
      }
    }
    if (s.holders !== holdersRef) {
      holdersRef = s.holders;
      overlays.setHolders(s.holders);
    }
    pushPieces();
    if (!m) return;
    const d = drag;
    if (d && !d.dead) {
      if (m.type === 'pieces_changed' && m.pieces.some((p) => p.id === d.anchor && p.held_by !== null && p.held_by === state.clientId)) {
        d.confirmed = true;
      } else if ((m.type === 'error' && !d.confirmed) || m.type === 'welcome') {
        d.dead = true; // refused, or a reconnect: the old lease is gone
        updateTurnButtons();
      }
    }
    switch (m.type) {
      case 'welcome':
      case 'snapshot':
        sync.apply(s.playback);
        restartClock();
        break;
      case 'playback_changed':
        sync.apply(m.playback);
        break;
      case 'pieces_changed':
        if (m.stats) restartClock();
        break;
      case 'cursor':
      case 'cursor_hidden':
        overlays.requestDraw('over');
        break;
      case 'ping':
        model.addPing(m.name, m.x, m.y, now());
        overlays.requestDraw('over');
        break;
      case 'error':
        showToast(m.message);
        break;
      default:
        break;
    }
  };
  cleanups.push(state.onChange(onMessage));
  cleanups.push(
    socket.onState((st) => {
      if (st !== 'open' && drag) drag.dead = true; // the server expires our leases
      netEl.textContent = st === 'open' ? '' : st === 'fatal' ? 'Disconnected' : 'Reconnecting…';
      overlays.requestDraw('over');
    }),
  );

  // First paint from what the room already has.
  const first = state.snapshot();
  if (first) {
    loadPuzzle(first);
    holdersRef = first.holders;
    overlays.setHolders(first.holders);
    pushPieces();
    sync.apply(first.playback);
    restartClock();
  }
  resize();

  let destroyed = false;
  return {
    camera,
    selection,
    setSelectMode,
    onSelectModeChange(cb) {
      selectCbs.push(cb);
      return () => {
        const i = selectCbs.indexOf(cb);
        if (i >= 0) selectCbs.splice(i, 1);
      };
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      // Let go of a held piece first (drag_end at its origin), while the room is still wired.
      detachPointer();
      machine.destroy();
      hideCursor();
      for (const c of cleanups.splice(0)) c();
      observer?.disconnect();
      observer = null;
      if (clockTimer !== null) timers.clear(clockTimer);
      if (toastTimer !== null) timers.clear(toastTimer);
      clockTimer = toastTimer = null;
      lod?.dispose();
      sync.dispose();
      overlays.dispose();
      renderer.dispose();
      try {
        video.pause();
        video.removeAttribute('src');
        video.load();
      } catch {
        // already gone
      }
      selectCbs.length = 0;
      section.remove();
    },
  };
}

/** The lobby app's entry point (see loadBoard.ts): a joined room, full screen. */
export const mountJoined: BoardModule['mountBoard'] = (root, joined, leave, config) =>
  mountBoard(root, {
    socket: joined.socket,
    state: joined.room,
    media: { assetBase: config.assetBase },
    spectator: joined.welcome.spectator,
    leave,
  });
