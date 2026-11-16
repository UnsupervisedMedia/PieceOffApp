// Pointer gesture state machine. Pure: it consumes PointerSample objects (screen
// CSS px relative to the canvas) and calls a GestureSink; no DOM, no real timers
// unless none are injected. `pointer.ts` adapts DOM PointerEvents to it.
//
// Decisions (see the Task 6 report for the reasoning):
// - Slop before a press becomes a drag/pan/rubber-band: touch 8 px, pen 4, mouse 3.
//   Within slop nothing is emitted, so taps and long-presses never jitter a piece.
// - Grab tolerance handed to `sink.hit`: touch 18 px, mouse and pen 10 px (desktop's
//   default grab margin). The sink converts px to table units (px / camera.scale).
// - Long-press (touch and pen only, default 450 ms) on a piece -> toggleSelect.
//   Moving beyond slop first cancels it. Moving after it fired drags that piece.
// - Double-tap/click on empty table (second press within 300 ms of the first
//   release, within 24 px) -> ping at the second tap. A third tap starts over.
// - Mouse: left on piece drags; left on empty rubber-bands (ctrl/meta/shift-less);
//   shift-left on empty, middle, right, or space-held left pans; ctrl/meta-click on
//   a piece toggles selection, ctrl/meta-drag on empty rubber-bands additively.
// - Touch/pen: on piece drags, on empty pans. Two touch/pen pointers pinch+pan.
//   A second finger during a drag sends exactly one dragCancel (the piece goes
//   back; no dragEnd), then pinches. When one of the two lifts, the remaining
//   finger pans from where it is (re-anchored: no jump, never resumes the drag,
//   never taps). Pointers beyond the second, and touch mixed with mouse, are ignored.
// - pointercancel: drag -> dragCancel, rubber-band -> rubberBand(null) with no
//   selectRect, anything else just stops.

export type PointerKind = 'mouse' | 'touch' | 'pen';

export interface PointerSample {
  pointerId: number;
  pointerType: PointerKind;
  /** Screen CSS px relative to the canvas's top-left. */
  x: number;
  y: number;
  /** PointerEvent.buttons: 1 primary/contact, 2 secondary, 4 middle. */
  buttons: number;
  ctrl: boolean;
  shift: boolean;
  meta: boolean;
  /** ms, monotonic (PointerEvent.timeStamp). */
  time: number;
}

export interface WheelSample {
  x: number;
  y: number;
  deltaY: number;
  /** WheelEvent.deltaMode: 0 px, 1 lines, 2 pages. */
  deltaMode: number;
  /** ctrl+wheel is how browsers report a trackpad pinch. */
  ctrl: boolean;
  /** px per page for deltaMode 2 (the view height). */
  pageSize?: number;
}

/** Table-space rectangle (w, h >= 0). */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** What the machine drives, and its view of the world: `hit` and `toTable` let it
 *  stay free of the camera and piece list (the sink owns both). */
export interface GestureSink {
  /** Piece id under screen point (sx, sy) with a grab tolerance of `tolerancePx`
   *  screen px (convert with / camera.scale), or null. Called once per press. */
  hit(sx: number, sy: number, tolerancePx: number): number | null;
  /** Screen CSS px -> table units. */
  toTable(sx: number, sy: number): [number, number];
  /** A drag of piece `id` began; (tx, ty) is the table point where it was pressed
   *  (the grab anchor: keep `piece - anchor` as the offset for dragMove). */
  dragStart(id: number, tx: number, ty: number): void;
  /** Pointer now at table point (tx, ty). */
  dragMove(tx: number, ty: number): void;
  dragEnd(): void;
  /** The drag is abandoned (second finger, pointercancel): put the piece back. */
  dragCancel(): void;
  /** Move the view by screen px. */
  pan(dx: number, dy: number): void;
  /** Zoom by `factor` about screen point (cx, cy). Sent after the matching pan. */
  pinch(cx: number, cy: number, factor: number): void;
  toggleSelect(id: number): void;
  /** Rubber-band overlay (table rect), or null to remove it. */
  rubberBand(rect: Rect | null): void;
  /** Rubber-band released over `rect`: select the pieces in it. `additive` when
   *  ctrl/meta was held at the press (add to the selection instead of replacing). */
  selectRect(rect: Rect, additive: boolean): void;
  ping(tx: number, ty: number): void;
  wheelZoom(sx: number, sy: number, factor: number): void;
}

export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface GestureOptions {
  longPressMs: number;
  doubleTapMs: number;
  /** Injected clock for long-press; defaults to setTimeout/clearTimeout. */
  timers: Timers;
}

export const TOUCH_TOLERANCE_PX = 18;
export const MOUSE_TOLERANCE_PX = 10;
export const PEN_TOLERANCE_PX = 10;
export const TOUCH_SLOP_PX = 8;
export const PEN_SLOP_PX = 4;
export const MOUSE_SLOP_PX = 3;
export const LONG_PRESS_MS = 450;
export const DOUBLE_TAP_MS = 300;
/** Max distance between the two taps of a double-tap. */
export const DOUBLE_TAP_PX = 24;

const tolerance = (k: PointerKind) => (k === 'touch' ? TOUCH_TOLERANCE_PX : k === 'pen' ? PEN_TOLERANCE_PX : MOUSE_TOLERANCE_PX);
const slop = (k: PointerKind) => (k === 'touch' ? TOUCH_SLOP_PX : k === 'pen' ? PEN_SLOP_PX : MOUSE_SLOP_PX);

/** Zoom factor for one wheel event: exp(-px * gain), clamped to [0.5, 2].
 *  Gain 0.0015/px for a wheel (one 100 px notch ~ x1.16), 0.01/px for ctrl+wheel
 *  (trackpad pinch reports small deltas). Lines count 16 px, pages `pageSize`. */
export function wheelFactor(deltaY: number, deltaMode: number, ctrl: boolean, pageSize = 800): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 1;
  const px = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * pageSize : deltaY;
  const f = Math.exp(-px * (ctrl ? 0.01 : 0.0015));
  return Math.min(2, Math.max(0.5, f));
}

type Kind = 'drag' | 'pan' | 'rubber';

interface Primary {
  id: number;
  type: PointerKind;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  startTime: number;
  /** What crossing the slop turns this press into. */
  kind: Kind;
  /** 'press' until slop is crossed (or immediately, after a pinch). */
  live: boolean;
  target: number | null;
  /** ctrl/meta at the press. */
  modifier: boolean;
  /** Can this press end as a tap/click (not a pinch leftover, no long-press fired)? */
  tappable: boolean;
  anchor: [number, number];
  rect: Rect | null;
}

interface Pinch {
  ids: [number, number];
  pos: Map<number, [number, number]>;
  types: Map<number, PointerKind>;
  cx: number;
  cy: number;
  dist: number;
}

const realTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

export class GestureMachine {
  private readonly longPressMs: number;
  private readonly doubleTapMs: number;
  private readonly timers: Timers;
  private primary: Primary | null = null;
  private pinchState: Pinch | null = null;
  private readonly ignored = new Set<number>();
  private longPress: unknown = null;
  private lastTap: { time: number; x: number; y: number } | null = null;
  private space = false;

  constructor(
    private readonly sink: GestureSink,
    opts?: Partial<GestureOptions>,
  ) {
    this.longPressMs = opts?.longPressMs ?? LONG_PRESS_MS;
    this.doubleTapMs = opts?.doubleTapMs ?? DOUBLE_TAP_MS;
    this.timers = opts?.timers ?? realTimers;
  }

  /** Space held: a left drag pans (desktop's space-drag). */
  setSpaceHeld(held: boolean): void {
    this.space = held;
  }

  wheel(w: WheelSample): void {
    const f = wheelFactor(w.deltaY, w.deltaMode, w.ctrl, w.pageSize);
    if (f !== 1) this.sink.wheelZoom(w.x, w.y, f);
  }

  pointerDown(s: PointerSample): void {
    const id = s.pointerId;
    if (this.ignored.has(id) || this.primary?.id === id || this.pinchState?.pos.has(id)) return;
    if (!this.primary && !this.pinchState) {
      this.begin(s);
      return;
    }
    const p = this.primary;
    if (this.pinchState || !p || p.type === 'mouse' || s.pointerType === 'mouse' || p.kind === 'rubber') {
      this.ignored.add(id);
      return;
    }
    // Second finger: whatever the first was doing becomes a pinch.
    this.clearLongPress();
    if (p.live && p.kind === 'drag') this.sink.dragCancel();
    this.primary = null;
    this.lastTap = null;
    const pos = new Map<number, [number, number]>([
      [p.id, [p.lastX, p.lastY]],
      [id, [s.x, s.y]],
    ]);
    const types = new Map<number, PointerKind>([
      [p.id, p.type],
      [id, s.pointerType],
    ]);
    this.pinchState = {
      ids: [p.id, id], pos, types,
      cx: (p.lastX + s.x) / 2, cy: (p.lastY + s.y) / 2,
      dist: Math.hypot(p.lastX - s.x, p.lastY - s.y),
    };
  }

  pointerMove(s: PointerSample): void {
    const id = s.pointerId;
    const pin = this.pinchState;
    if (pin?.pos.has(id)) {
      pin.pos.set(id, [s.x, s.y]);
      const [a, b] = pin.ids.map((k) => pin.pos.get(k)!);
      const cx = (a[0] + b[0]) / 2;
      const cy = (a[1] + b[1]) / 2;
      const dist = Math.hypot(a[0] - b[0], a[1] - b[1]);
      if (cx !== pin.cx || cy !== pin.cy) this.sink.pan(cx - pin.cx, cy - pin.cy);
      if (pin.dist > 0 && dist > 0 && dist !== pin.dist) this.sink.pinch(cx, cy, dist / pin.dist);
      pin.cx = cx;
      pin.cy = cy;
      if (dist > 0) pin.dist = dist;
      return;
    }
    const p = this.primary;
    if (!p || p.id !== id) return;
    if (!p.live) {
      if (Math.hypot(s.x - p.startX, s.y - p.startY) <= slop(p.type)) return;
      this.clearLongPress();
      p.live = true;
      p.tappable = false;
      if (p.kind === 'drag') this.sink.dragStart(p.target!, p.anchor[0], p.anchor[1]);
    }
    if (p.kind === 'drag') {
      const [tx, ty] = this.sink.toTable(s.x, s.y);
      this.sink.dragMove(tx, ty);
    } else if (p.kind === 'pan') {
      this.sink.pan(s.x - p.lastX, s.y - p.lastY);
    } else {
      const [tx, ty] = this.sink.toTable(s.x, s.y);
      const [ax, ay] = p.anchor;
      p.rect = { x: Math.min(ax, tx), y: Math.min(ay, ty), w: Math.abs(tx - ax), h: Math.abs(ty - ay) };
      this.sink.rubberBand(p.rect);
    }
    p.lastX = s.x;
    p.lastY = s.y;
  }

  pointerUp(s: PointerSample): void {
    this.end(s, false);
  }

  pointerCancel(s: PointerSample): void {
    this.end(s, true);
  }

  private begin(s: PointerSample): void {
    const type = s.pointerType;
    const modifier = s.ctrl || s.meta;
    const left = (s.buttons & 1) !== 0 || s.buttons === 0;
    let kind: Kind;
    let target: number | null = null;
    if (type === 'mouse' && (!left || this.space)) {
      kind = 'pan';
    } else {
      target = this.sink.hit(s.x, s.y, tolerance(type));
      if (target !== null) kind = 'drag';
      else if (type !== 'mouse' || (s.shift && !modifier)) kind = 'pan';
      else kind = 'rubber';
    }
    this.primary = {
      id: s.pointerId, type, startX: s.x, startY: s.y, lastX: s.x, lastY: s.y, startTime: s.time,
      kind, live: false, target, modifier, tappable: left && !this.space, anchor: this.sink.toTable(s.x, s.y), rect: null,
    };
    if (target !== null && type !== 'mouse') {
      const p = this.primary;
      this.longPress = this.timers.set(() => {
        this.longPress = null;
        if (this.primary !== p || p.live) return;
        p.tappable = false;
        this.lastTap = null;
        this.sink.toggleSelect(p.target!);
      }, this.longPressMs);
    }
  }

  private end(s: PointerSample, cancelled: boolean): void {
    const id = s.pointerId;
    if (this.ignored.delete(id)) return;
    const pin = this.pinchState;
    if (pin?.pos.has(id)) {
      // Pinch over: the other finger pans from where it is. Not a new press.
      const other = pin.ids[0] === id ? pin.ids[1] : pin.ids[0];
      const [x, y] = pin.pos.get(other)!;
      this.pinchState = null;
      this.primary = {
        id: other, type: pin.types.get(other)!, startX: x, startY: y, lastX: x, lastY: y, startTime: s.time,
        kind: 'pan', live: true, target: null, modifier: false, tappable: false, anchor: [0, 0], rect: null,
      };
      return;
    }
    const p = this.primary;
    if (!p || p.id !== id) return;
    this.clearLongPress();
    this.primary = null;
    if (cancelled) {
      this.lastTap = null;
      if (p.live && p.kind === 'drag') this.sink.dragCancel();
      else if (p.live && p.kind === 'rubber') this.sink.rubberBand(null);
      return;
    }
    if (p.live) {
      if (p.kind === 'drag') this.sink.dragEnd();
      else if (p.kind === 'rubber') {
        this.sink.rubberBand(null);
        if (p.rect) this.sink.selectRect(p.rect, p.modifier);
      }
      this.lastTap = null;
      return;
    }
    if (!p.tappable) return;
    // A click/tap.
    if (p.target !== null) {
      this.lastTap = null;
      if (p.modifier && p.type !== 'touch') this.sink.toggleSelect(p.target);
      return;
    }
    if (p.modifier || s.time - p.startTime >= this.longPressMs) {
      this.lastTap = null;
      return;
    }
    const prev = this.lastTap;
    if (prev && p.startTime - prev.time <= this.doubleTapMs && Math.hypot(p.startX - prev.x, p.startY - prev.y) <= DOUBLE_TAP_PX) {
      this.lastTap = null;
      const [tx, ty] = this.sink.toTable(p.startX, p.startY);
      this.sink.ping(tx, ty);
    } else {
      this.lastTap = { time: s.time, x: p.startX, y: p.startY };
    }
  }

  private clearLongPress(): void {
    if (this.longPress !== null) this.timers.clear(this.longPress);
    this.longPress = null;
  }
}
