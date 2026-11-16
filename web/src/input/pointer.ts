// Thin DOM adapter: PointerEvents/wheel/keys on a canvas -> GestureMachine.
// Coordinates are relative to the element's getBoundingClientRect (the canvas
// need not sit at the page origin), in CSS px like Camera's screen space.
import type { Camera } from '../gl/camera';
import type { PointerKind, PointerSample, WheelSample } from './gestures';

/** The part of GestureMachine the adapter drives. */
export interface PointerTarget {
  pointerDown(s: PointerSample): void;
  pointerMove(s: PointerSample): void;
  pointerUp(s: PointerSample): void;
  pointerCancel(s: PointerSample): void;
  wheel(w: WheelSample): void;
  setSpaceHeld(held: boolean): void;
  /** Forget every pointer (cancelling a live drag); called on detach. */
  reset(): void;
}

const kind = (t: string): PointerKind => (t === 'touch' || t === 'pen' ? t : 'mouse');

const typing = (t: EventTarget | null): boolean =>
  t instanceof HTMLElement && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));

/** Wire `el` to `machine`. `camera` supplies the page size for page-mode wheel
 *  deltas. Sets `touch-action: none` (and kills overscroll/selection/callouts)
 *  so the browser does not pan, zoom, back-swipe or pull-to-refresh under the
 *  game. Returns a detach function, which also `reset()`s the machine (a live drag
 *  is cancelled, timers cleared). */
export function attachPointerEvents(el: HTMLElement, machine: PointerTarget, camera: Camera): () => void {
  const st = el.style as CSSStyleDeclaration & { webkitUserSelect?: string; webkitTouchCallout?: string };
  st.touchAction = 'none';
  st.userSelect = 'none';
  st.webkitUserSelect = 'none';
  st.webkitTouchCallout = 'none';
  st.overscrollBehavior = 'none';

  const active = new Set<number>();
  const sample = (e: PointerEvent): PointerSample => {
    const r = el.getBoundingClientRect();
    return {
      pointerId: e.pointerId,
      pointerType: kind(e.pointerType),
      x: e.clientX - r.left,
      y: e.clientY - r.top,
      buttons: e.buttons,
      ctrl: e.ctrlKey,
      shift: e.shiftKey,
      meta: e.metaKey,
      time: e.timeStamp,
    };
  };

  const down = (e: PointerEvent) => {
    e.preventDefault();
    try {
      el.setPointerCapture(e.pointerId);
    } catch {
      /* pointer already gone */
    }
    active.add(e.pointerId);
    machine.pointerDown(sample(e));
  };
  const move = (e: PointerEvent) => {
    if (active.has(e.pointerId)) machine.pointerMove(sample(e));
  };
  const up = (e: PointerEvent) => {
    if (!active.delete(e.pointerId)) return;
    machine.pointerUp(sample(e));
  };
  // pointercancel, or capture lost without an up (element removed, etc.).
  const cancel = (e: PointerEvent) => {
    if (!active.delete(e.pointerId)) return;
    machine.pointerCancel(sample(e));
  };
  const wheel = (e: WheelEvent) => {
    e.preventDefault();
    const r = el.getBoundingClientRect();
    machine.wheel({ x: e.clientX - r.left, y: e.clientY - r.top, deltaY: e.deltaY, deltaMode: e.deltaMode, ctrl: e.ctrlKey, pageSize: camera.viewH });
  };
  const noMenu = (e: Event) => e.preventDefault();
  const keyDown = (e: KeyboardEvent) => {
    if (e.code !== 'Space' || typing(e.target)) return;
    if (!e.repeat) machine.setSpaceHeld(true);
    // Stop the page scrolling, but leave space on focused buttons alone.
    if (e.target === el || e.target === document.body) e.preventDefault();
  };
  const keyUp = (e: KeyboardEvent) => {
    if (e.code === 'Space' && !typing(e.target)) machine.setSpaceHeld(false);
  };
  const blur = () => machine.setSpaceHeld(false);

  const on: [EventTarget, string, EventListener, AddEventListenerOptions?][] = [
    [el, 'pointerdown', down as EventListener],
    [el, 'pointermove', move as EventListener],
    [el, 'pointerup', up as EventListener],
    [el, 'pointercancel', cancel as EventListener],
    [el, 'lostpointercapture', cancel as EventListener],
    [el, 'wheel', wheel as EventListener, { passive: false }],
    [el, 'contextmenu', noMenu],
    // iOS Safari page pinch-zoom.
    [el, 'gesturestart', noMenu],
    [window, 'keydown', keyDown as EventListener],
    [window, 'keyup', keyUp as EventListener],
    [window, 'blur', blur],
  ];
  for (const [t, n, f, o] of on) t.addEventListener(n, f, o);
  return () => {
    for (const [t, n, f] of on) t.removeEventListener(n, f);
    active.clear();
    machine.reset();
  };
}
