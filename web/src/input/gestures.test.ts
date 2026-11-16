// @vitest-environment node
import { describe, expect, it } from 'vitest';
import {
  GestureMachine,
  MOUSE_TOLERANCE_PX,
  PEN_TOLERANCE_PX,
  TOUCH_TOLERANCE_PX,
  wheelFactor,
  type GestureSink,
  type PointerSample,
  type Timers,
} from './gestures';

type Call = [string, ...unknown[]];

/** Screen layout used by every test: a piece (id 7) covers x < 100, the rest is empty table.
 *  Table = screen / 2 so the tests see the mapping is applied. */
function recorder(): { sink: GestureSink; calls: Call[]; hits: [number, number, number][] } {
  const calls: Call[] = [];
  const hits: [number, number, number][] = [];
  const rec = (name: string) => (...args: unknown[]) => void calls.push([name, ...args]);
  const sink: GestureSink = {
    hit(sx, sy, tolPx) {
      hits.push([sx, sy, tolPx]);
      return sx < 100 ? 7 : null;
    },
    toTable: (sx, sy) => [sx / 2, sy / 2],
    dragStart: rec('dragStart'),
    dragMove: rec('dragMove'),
    dragEnd: rec('dragEnd'),
    dragCancel: rec('dragCancel'),
    pan: rec('pan'),
    pinch: rec('pinch'),
    toggleSelect: rec('toggleSelect'),
    rubberBand: rec('rubberBand'),
    selectRect: rec('selectRect'),
    ping: rec('ping'),
    wheelZoom: rec('wheelZoom'),
  };
  return { sink, calls, hits };
}

/** Manual clock: timers fire only when the test advances it. */
function fakeTimers(): Timers & { advance(ms: number): void; now: number } {
  let now = 0;
  let next = 1;
  const pending = new Map<number, { at: number; fn: () => void }>();
  return {
    get now() {
      return now;
    },
    set(fn, ms) {
      const h = next++;
      pending.set(h, { at: now + ms, fn });
      return h;
    },
    clear(h) {
      pending.delete(h as number);
    },
    advance(ms) {
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
    },
  };
}

type Kind = 'mouse' | 'touch' | 'pen';
type Step =
  | ['down' | 'move' | 'up' | 'cancel', number, number, number, Partial<PointerSample>?]
  | ['wait', number];

/** Run a synthetic sequence. Pointer id 1..n; pointerType from `kind` unless overridden. */
function run(kind: Kind, steps: Step[], opts?: { longPressMs?: number; doubleTapMs?: number }) {
  const r = recorder();
  const t = fakeTimers();
  const m = new GestureMachine(r.sink, { ...opts, timers: t });
  for (const st of steps) {
    if (st[0] === 'wait') {
      t.advance(st[1]);
      continue;
    }
    const [ev, id, x, y, extra] = st;
    const s: PointerSample = {
      pointerId: id, pointerType: kind, x, y,
      buttons: ev === 'up' || ev === 'cancel' ? 0 : 1,
      ctrl: false, shift: false, meta: false, time: t.now, ...extra,
    };
    if (ev === 'down') m.pointerDown(s);
    else if (ev === 'move') m.pointerMove(s);
    else if (ev === 'up') m.pointerUp(s);
    else m.pointerCancel(s);
    t.advance(16);
  }
  return { ...r, machine: m };
}

const names = (calls: Call[]) => calls.map((c) => c[0]);

describe('GestureMachine', () => {
  const table: { name: string; kind: Kind; steps: Step[]; want: Call[] }[] = [
    {
      name: 'one_finger_on_piece_drags',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['move', 1, 60, 50], ['move', 1, 80, 70], ['up', 1, 80, 70]],
      want: [['dragStart', 7, 25, 25], ['dragMove', 30, 25], ['dragMove', 40, 35], ['dragEnd']],
    },
    {
      name: 'mouse_on_piece_drags',
      kind: 'mouse',
      steps: [['down', 1, 50, 50], ['move', 1, 54, 50], ['up', 1, 54, 50]],
      want: [['dragStart', 7, 25, 25], ['dragMove', 27, 25], ['dragEnd']],
    },
    {
      name: 'one_finger_on_empty_pans',
      kind: 'touch',
      steps: [['down', 1, 200, 200], ['move', 1, 212, 200], ['move', 1, 220, 190], ['up', 1, 220, 190]],
      want: [['pan', 12, 0], ['pan', 8, -10]],
    },
    {
      name: 'touch_slop_swallows_jitter',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['move', 1, 55, 53], ['move', 1, 47, 46], ['up', 1, 47, 46]],
      want: [],
    },
    {
      name: 'mouse_drag_on_empty_rubber_bands',
      kind: 'mouse',
      steps: [['down', 1, 300, 100], ['move', 1, 250, 140], ['move', 1, 200, 160], ['up', 1, 200, 160]],
      want: [
        ['rubberBand', { x: 125, y: 50, w: 25, h: 20 }],
        ['rubberBand', { x: 100, y: 50, w: 50, h: 30 }],
        ['rubberBand', null],
        ['selectRect', { x: 100, y: 50, w: 50, h: 30 }, false],
      ],
    },
    {
      name: 'ctrl_mouse_rubber_band_is_additive',
      kind: 'mouse',
      steps: [['down', 1, 300, 100, { ctrl: true }], ['move', 1, 320, 120, { ctrl: true }], ['up', 1, 320, 120]],
      want: [['rubberBand', { x: 150, y: 50, w: 10, h: 10 }], ['rubberBand', null], ['selectRect', { x: 150, y: 50, w: 10, h: 10 }, true]],
    },
    {
      name: 'right_drag_pans_even_over_a_piece',
      kind: 'mouse',
      steps: [['down', 1, 50, 50, { buttons: 2 }], ['move', 1, 60, 50, { buttons: 2 }], ['up', 1, 60, 50]],
      want: [['pan', 10, 0]],
    },
    {
      name: 'middle_drag_pans',
      kind: 'mouse',
      steps: [['down', 1, 300, 50, { buttons: 4 }], ['move', 1, 300, 70, { buttons: 4 }], ['up', 1, 300, 70]],
      want: [['pan', 0, 20]],
    },
    {
      name: 'shift_drag_on_empty_pans',
      kind: 'mouse',
      steps: [['down', 1, 300, 50, { shift: true }], ['move', 1, 310, 50, { shift: true }], ['up', 1, 310, 50]],
      want: [['pan', 10, 0]],
    },
    {
      name: 'two_fingers_pinch_and_pan',
      kind: 'touch',
      steps: [
        ['down', 1, 200, 200], ['down', 2, 300, 200],
        ['move', 2, 400, 200], // spread: centroid 250 -> 300, distance 100 -> 200
        ['move', 1, 300, 200], // centroid 300 -> 350, distance 200 -> 100
        ['up', 2, 400, 200], ['up', 1, 300, 200],
      ],
      want: [['pan', 50, 0], ['pinch', 300, 200, 2], ['pan', 50, 0], ['pinch', 350, 200, 0.5]],
    },
    {
      name: 'second_finger_during_drag_cancels_drag_then_pinches',
      kind: 'touch',
      steps: [
        ['down', 1, 50, 50], ['move', 1, 70, 50], // drag piece 7
        ['down', 2, 170, 50], // second finger: cancel the drag, start a pinch
        ['move', 2, 270, 50], // centroid 120 -> 170, distance 100 -> 200
        ['up', 2, 270, 50], // pinch ends; finger 1 is left
        ['move', 1, 80, 60], // finger 1 pans from where it is: no drag, no jump
        ['up', 1, 80, 60],
      ],
      want: [['dragStart', 7, 25, 25], ['dragMove', 35, 25], ['dragCancel'], ['pan', 50, 0], ['pinch', 170, 50, 2], ['pan', 10, 10]],
    },
    {
      name: 'pinch_from_empty_then_lift_first_finger_pans_with_second',
      kind: 'touch',
      steps: [
        ['down', 1, 200, 200], ['down', 2, 300, 200], ['up', 1, 200, 200],
        ['move', 2, 305, 200], // below slop, but no slop after a pinch: it is already a pan
        ['up', 2, 305, 200],
      ],
      want: [['pan', 5, 0]],
    },
    {
      name: 'third_finger_is_ignored',
      kind: 'touch',
      steps: [
        ['down', 1, 200, 200], ['down', 2, 300, 200], ['down', 3, 500, 500],
        ['move', 3, 600, 600], ['up', 3, 600, 600],
        ['move', 2, 400, 200],
        ['up', 1, 200, 200], ['up', 2, 400, 200],
      ],
      want: [['pan', 50, 0], ['pinch', 300, 200, 2]],
    },
    {
      name: 'long_press_on_piece_toggles_select',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['wait', 500], ['up', 1, 50, 50]],
      want: [['toggleSelect', 7]],
    },
    {
      name: 'long_press_then_drag_drags_from_the_press_point',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['wait', 500], ['move', 1, 70, 50], ['up', 1, 70, 50]],
      want: [['toggleSelect', 7], ['dragStart', 7, 25, 25], ['dragMove', 35, 25], ['dragEnd']],
    },
    {
      name: 'moving_beyond_slop_cancels_long_press',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['wait', 200], ['move', 1, 70, 50], ['wait', 500], ['up', 1, 70, 50]],
      want: [['dragStart', 7, 25, 25], ['dragMove', 35, 25], ['dragEnd']],
    },
    {
      name: 'short_press_on_piece_is_not_a_long_press',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['wait', 300], ['up', 1, 50, 50], ['wait', 1000]],
      want: [],
    },
    {
      name: 'long_press_on_empty_table_does_nothing',
      kind: 'touch',
      steps: [['down', 1, 200, 50], ['wait', 600], ['up', 1, 200, 50]],
      want: [],
    },
    {
      name: 'double_tap_on_empty_pings',
      kind: 'touch',
      steps: [['down', 1, 200, 50], ['up', 1, 200, 50], ['wait', 100], ['down', 2, 204, 52], ['up', 2, 204, 52]],
      want: [['ping', 102, 26]],
    },
    {
      name: 'slow_double_tap_does_not_ping',
      kind: 'touch',
      steps: [['down', 1, 200, 50], ['up', 1, 200, 50], ['wait', 400], ['down', 2, 200, 50], ['up', 2, 200, 50]],
      want: [],
    },
    {
      name: 'far_apart_taps_do_not_ping',
      kind: 'touch',
      steps: [['down', 1, 200, 50], ['up', 1, 200, 50], ['down', 2, 300, 50], ['up', 2, 300, 50]],
      want: [],
    },
    {
      name: 'triple_tap_pings_once',
      kind: 'touch',
      steps: [['down', 1, 200, 50], ['up', 1, 200, 50], ['down', 2, 200, 50], ['up', 2, 200, 50], ['down', 3, 200, 50], ['up', 3, 200, 50]],
      want: [['ping', 100, 25]],
    },
    {
      name: 'double_tap_on_piece_does_not_ping',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['up', 1, 50, 50], ['down', 2, 50, 50], ['up', 2, 50, 50]],
      want: [],
    },
    {
      name: 'double_click_on_empty_pings',
      kind: 'mouse',
      steps: [['down', 1, 200, 50], ['up', 1, 200, 50], ['down', 1, 200, 50], ['up', 1, 200, 50]],
      want: [['ping', 100, 25]],
    },
    {
      name: 'ctrl_click_toggles_select',
      kind: 'mouse',
      steps: [['down', 1, 50, 50, { ctrl: true }], ['up', 1, 50, 50, { ctrl: true }]],
      want: [['toggleSelect', 7]],
    },
    {
      name: 'meta_click_toggles_select',
      kind: 'mouse',
      steps: [['down', 1, 50, 50, { meta: true }], ['up', 1, 51, 50, { meta: true }]],
      want: [['toggleSelect', 7]],
    },
    {
      name: 'plain_click_on_piece_does_nothing',
      kind: 'mouse',
      steps: [['down', 1, 50, 50], ['up', 1, 50, 50]],
      want: [],
    },
    {
      name: 'mouse_does_not_long_press',
      kind: 'mouse',
      steps: [['down', 1, 50, 50], ['wait', 1000], ['up', 1, 50, 50]],
      want: [],
    },
    {
      name: 'pointercancel_mid_drag_cancels_drag',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['move', 1, 70, 50], ['cancel', 1, 70, 50], ['move', 1, 90, 50], ['up', 1, 90, 50]],
      want: [['dragStart', 7, 25, 25], ['dragMove', 35, 25], ['dragCancel']],
    },
    {
      name: 'pointercancel_mid_rubber_band_clears_without_selecting',
      kind: 'mouse',
      steps: [['down', 1, 300, 100], ['move', 1, 320, 120], ['cancel', 1, 320, 120]],
      want: [['rubberBand', { x: 150, y: 50, w: 10, h: 10 }], ['rubberBand', null]],
    },
    {
      name: 'pointercancel_before_slop_is_silent_and_kills_long_press',
      kind: 'touch',
      steps: [['down', 1, 50, 50], ['cancel', 1, 50, 50], ['wait', 1000]],
      want: [],
    },
    {
      name: 'mouse_plus_touch_does_not_pinch',
      kind: 'mouse',
      steps: [
        ['down', 1, 50, 50], ['move', 1, 60, 50],
        ['down', 2, 300, 300, { pointerType: 'touch' }], ['move', 2, 400, 300, { pointerType: 'touch' }], ['up', 2, 400, 300, { pointerType: 'touch' }],
        ['up', 1, 60, 50],
      ],
      want: [['dragStart', 7, 25, 25], ['dragMove', 30, 25], ['dragEnd']],
    },
    {
      name: 'unknown_pointer_events_are_ignored',
      kind: 'touch',
      steps: [['move', 9, 1, 1], ['up', 9, 1, 1], ['cancel', 9, 1, 1]],
      want: [],
    },
  ];

  for (const c of table) {
    it(c.name, () => {
      expect(run(c.kind, c.steps).calls).toEqual(c.want);
    });
  }

  it('second_finger_during_drag_emits_exactly_one_dragCancel_and_no_dragEnd', () => {
    const { calls } = run('touch', [
      ['down', 1, 50, 50], ['move', 1, 70, 50], ['move', 1, 75, 50],
      ['down', 2, 170, 50], ['move', 2, 200, 50], ['move', 1, 60, 50],
      ['up', 1, 60, 50], ['move', 2, 220, 70], ['up', 2, 220, 70],
    ]);
    const n = names(calls);
    expect(n.filter((x) => x === 'dragCancel')).toHaveLength(1);
    expect(n).not.toContain('dragEnd');
    // Nothing drag-shaped after the cancel.
    const after = n.slice(n.indexOf('dragCancel') + 1);
    expect(after.some((x) => x.startsWith('drag'))).toBe(false);
    expect(after).toContain('pinch');
    expect(after[after.length - 1]).toBe('pan'); // finger 2 left alone pans
  });

  it('finger_tolerance_is_larger_for_touch_than_mouse', () => {
    expect(TOUCH_TOLERANCE_PX).toBeGreaterThan(MOUSE_TOLERANCE_PX);
    expect(TOUCH_TOLERANCE_PX).toBeGreaterThan(PEN_TOLERANCE_PX);
    const t = run('touch', [['down', 1, 50, 50], ['up', 1, 50, 50]]);
    const mo = run('mouse', [['down', 1, 50, 50], ['up', 1, 50, 50]]);
    const pe = run('pen', [['down', 1, 50, 50], ['up', 1, 50, 50]]);
    expect(t.hits).toEqual([[50, 50, TOUCH_TOLERANCE_PX]]);
    expect(mo.hits).toEqual([[50, 50, MOUSE_TOLERANCE_PX]]);
    expect(pe.hits).toEqual([[50, 50, PEN_TOLERANCE_PX]]);
  });

  it('long_press_and_double_tap_timings_are_configurable', () => {
    expect(run('touch', [['down', 1, 50, 50], ['wait', 150], ['up', 1, 50, 50]], { longPressMs: 100 }).calls).toEqual([['toggleSelect', 7]]);
    expect(run('touch', [['down', 1, 50, 50], ['wait', 420], ['up', 1, 50, 50]]).calls).toEqual([]);
    expect(run('touch', [['down', 1, 50, 50], ['wait', 450], ['up', 1, 50, 50]]).calls).toEqual([['toggleSelect', 7]]);
    const slow: Step[] = [['down', 1, 200, 50], ['up', 1, 200, 50], ['wait', 400], ['down', 2, 200, 50], ['up', 2, 200, 50]];
    expect(run('touch', slow, { doubleTapMs: 600 }).calls).toEqual([['ping', 100, 25]]);
  });

  it('pen_behaves_like_touch_for_empty_table_and_long_press', () => {
    expect(run('pen', [['down', 1, 200, 200], ['move', 1, 220, 200], ['up', 1, 220, 200]]).calls).toEqual([['pan', 20, 0]]);
    expect(run('pen', [['down', 1, 50, 50], ['wait', 500], ['up', 1, 50, 50]]).calls).toEqual([['toggleSelect', 7]]);
  });

  it('space_held_makes_left_drag_pan', () => {
    const r = recorder();
    const m = new GestureMachine(r.sink, { timers: fakeTimers() });
    m.setSpaceHeld(true);
    const s = (x: number, buttons = 1): PointerSample => ({ pointerId: 1, pointerType: 'mouse', x, y: 50, buttons, ctrl: false, shift: false, meta: false, time: 0 });
    m.pointerDown(s(50));
    m.pointerMove(s(70));
    m.pointerUp(s(70, 0));
    expect(r.calls).toEqual([['pan', 20, 0]]);
  });

  it('wheel_zooms_about_the_pointer', () => {
    const r = recorder();
    const m = new GestureMachine(r.sink, { timers: fakeTimers() });
    m.wheel({ x: 10, y: 20, deltaY: -100, deltaMode: 0, ctrl: false });
    expect(r.calls).toHaveLength(1);
    const [name, sx, sy, f] = r.calls[0];
    expect([name, sx, sy]).toEqual(['wheelZoom', 10, 20]);
    expect(f).toBeCloseTo(wheelFactor(-100, 0, false));
  });

  it('wheel_factor_is_sane', () => {
    // One notch (100 px) zooms in by roughly 15%; down zooms out by the inverse.
    const inF = wheelFactor(-100, 0, false);
    expect(inF).toBeGreaterThan(1.1);
    expect(inF).toBeLessThan(1.25);
    expect(wheelFactor(100, 0, false)).toBeCloseTo(1 / inF);
    expect(wheelFactor(0, 0, false)).toBe(1);
    // Line mode (Firefox): 3 lines ~ one notch.
    expect(wheelFactor(-3, 1, false)).toBeCloseTo(wheelFactor(-48, 0, false));
    // Trackpad pinch arrives as ctrl+wheel with small deltas: more gain per px.
    expect(wheelFactor(-10, 0, true)).toBeGreaterThan(wheelFactor(-10, 0, false));
    // Huge deltas are clamped.
    expect(wheelFactor(-1e6, 0, false)).toBe(2);
    expect(wheelFactor(1e6, 0, false)).toBe(0.5);
    expect(wheelFactor(Number.NaN, 0, false)).toBe(1);
  });
});
