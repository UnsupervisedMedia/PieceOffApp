import { describe, expect, it, vi } from 'vitest';
import { Camera } from '../gl/camera';
import type { PointerSample, WheelSample } from './gestures';
import { attachPointerEvents, type PointerTarget } from './pointer';

function fakeMachine() {
  const calls: [string, PointerSample | WheelSample | boolean][] = [];
  const m: PointerTarget = {
    pointerDown: (s) => void calls.push(['down', s]),
    pointerMove: (s) => void calls.push(['move', s]),
    pointerUp: (s) => void calls.push(['up', s]),
    pointerCancel: (s) => void calls.push(['cancel', s]),
    wheel: (s) => void calls.push(['wheel', s]),
    setSpaceHeld: (b) => void calls.push(['space', b]),
    reset: () => void calls.push(['reset', false]),
  };
  return { m, calls };
}

function canvasAt(left: number, top: number): HTMLCanvasElement {
  const el = document.createElement('canvas');
  document.body.appendChild(el);
  el.getBoundingClientRect = () => ({ left, top, x: left, y: top, width: 300, height: 200, right: left + 300, bottom: top + 200, toJSON() {} }) as DOMRect;
  el.setPointerCapture = vi.fn();
  el.releasePointerCapture = vi.fn();
  return el;
}

function pe(type: string, init: PointerEventInit): PointerEvent {
  return new PointerEvent(type, { bubbles: true, cancelable: true, ...init });
}

describe('attachPointerEvents', () => {
  it('maps_client_coords_through_getBoundingClientRect', () => {
    const el = canvasAt(100, 50);
    const { m, calls } = fakeMachine();
    attachPointerEvents(el, m, new Camera(300, 200));
    el.dispatchEvent(pe('pointerdown', { pointerId: 3, pointerType: 'touch', clientX: 110, clientY: 70, buttons: 1, ctrlKey: true }));
    expect(calls).toHaveLength(1);
    const [name, s] = calls[0] as [string, PointerSample];
    expect(name).toBe('down');
    expect(s).toMatchObject({ pointerId: 3, pointerType: 'touch', x: 10, y: 20, buttons: 1, ctrl: true, shift: false, meta: false });
    expect(el.setPointerCapture).toHaveBeenCalledWith(3);
  });

  it('sets_touch_action_none_and_suppresses_contextmenu', () => {
    const el = canvasAt(0, 0);
    attachPointerEvents(el, fakeMachine().m, new Camera(300, 200));
    expect(el.style.touchAction).toBe('none');
    const ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    el.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
  });

  it('routes_up_cancel_and_lost_capture', () => {
    const el = canvasAt(0, 0);
    const { m, calls } = fakeMachine();
    attachPointerEvents(el, m, new Camera(300, 200));
    el.dispatchEvent(pe('pointerdown', { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, buttons: 1 }));
    el.dispatchEvent(pe('pointermove', { pointerId: 1, pointerType: 'mouse', clientX: 5, clientY: 1, buttons: 1 }));
    el.dispatchEvent(pe('pointerup', { pointerId: 1, pointerType: 'mouse', clientX: 5, clientY: 1 }));
    // lostpointercapture after a normal up is not a cancel.
    el.dispatchEvent(pe('lostpointercapture', { pointerId: 1, pointerType: 'mouse' }));
    el.dispatchEvent(pe('pointerdown', { pointerId: 2, pointerType: 'touch', clientX: 1, clientY: 1, buttons: 1 }));
    el.dispatchEvent(pe('pointercancel', { pointerId: 2, pointerType: 'touch' }));
    el.dispatchEvent(pe('pointerdown', { pointerId: 4, pointerType: 'touch', clientX: 1, clientY: 1, buttons: 1 }));
    el.dispatchEvent(pe('lostpointercapture', { pointerId: 4, pointerType: 'touch' }));
    expect(calls.map((c) => c[0] + ':' + (c[1] as PointerSample).pointerId)).toEqual([
      'down:1', 'move:1', 'up:1', 'down:2', 'cancel:2', 'down:4', 'cancel:4',
    ]);
  });

  it('wheel_is_forwarded_with_ctrl_and_prevented', () => {
    const el = canvasAt(100, 50);
    const { m, calls } = fakeMachine();
    attachPointerEvents(el, m, new Camera(300, 200));
    const ev = new WheelEvent('wheel', { bubbles: true, cancelable: true, clientX: 150, clientY: 60, deltaY: -3, deltaMode: 1, ctrlKey: true });
    el.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(calls).toEqual([['wheel', { x: 50, y: 10, deltaY: -3, deltaMode: 1, ctrl: true, pageSize: 200 }]]);
  });

  it('space_key_toggles_pan_mode_but_not_in_text_fields', () => {
    const el = canvasAt(0, 0);
    const { m, calls } = fakeMachine();
    attachPointerEvents(el, m, new Camera(300, 200));
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'Space', bubbles: true }));
    window.dispatchEvent(new KeyboardEvent('keyup', { code: 'Space', bubbles: true }));
    const input = document.createElement('input');
    document.body.appendChild(input);
    input.dispatchEvent(new KeyboardEvent('keydown', { code: 'Space', bubbles: true }));
    expect(calls).toEqual([['space', true], ['space', false]]);
  });

  it('detach_removes_listeners', () => {
    const el = canvasAt(0, 0);
    const { m, calls } = fakeMachine();
    const detach = attachPointerEvents(el, m, new Camera(300, 200));
    detach();
    el.dispatchEvent(pe('pointerdown', { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, buttons: 1 }));
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'Space', bubbles: true }));
    expect(calls.map((c) => c[0])).toEqual(['reset']); // only detach's own reset
  });

  it('detach_resets_the_machine', () => {
    const el = canvasAt(0, 0);
    const { m, calls } = fakeMachine();
    const detach = attachPointerEvents(el, m, new Camera(300, 200));
    detach();
    expect(calls.map((c) => c[0])).toEqual(['reset']);
    el.dispatchEvent(pe('pointerdown', { pointerId: 1, pointerType: 'mouse', clientX: 1, clientY: 1, buttons: 1 }));
    expect(calls.map((c) => c[0])).toEqual(['reset']);
  });
});
