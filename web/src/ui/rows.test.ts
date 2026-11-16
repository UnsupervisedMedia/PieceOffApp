import { afterEach, describe, expect, it, vi } from 'vitest';
import { patchRows, type KeyedRow } from './rows';

afterEach(() => document.body.replaceChildren());

const row = (key: string, sig = key): KeyedRow => ({
  key,
  sig,
  build: () => {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.dataset.act = 'go';
    b.textContent = `${key}:${sig}`;
    li.append(b);
    return li;
  },
});

function list(keys: string[]) {
  const ul = document.createElement('ul');
  document.body.append(ul);
  patchRows(ul, keys.map((k) => row(k)));
  return ul;
}
const keysOf = (ul: HTMLElement) => [...ul.children].map((c) => (c as HTMLElement).dataset.key);

describe('patchRows', () => {
  it('keeps_kept_nodes_and_never_moves_placed_ones_on_removal_or_insertion', () => {
    const cases: [string[], string[]][] = [
      [['a', 'b', 'c', 'd'], ['b', 'c', 'd']], // remove first
      [['a', 'b', 'c', 'd'], ['a', 'c', 'd']], // remove middle
      [['a', 'b', 'c', 'd'], ['a', 'b', 'c']], // remove last
      [['a', 'b', 'c'], ['x', 'a', 'b', 'c']], // add first
      [['a', 'b', 'c'], ['a', 'x', 'b', 'c']], // add middle
      [['a', 'b', 'c'], ['a', 'b', 'c', 'x']], // add last
      [['a', 'b', 'c', 'd'], ['a', 'x', 'c', 'd', 'y']], // remove and add
    ];
    for (const [before, after] of cases) {
      const ul = list(before);
      const nodes = new Map([...ul.children].map((c) => [(c as HTMLElement).dataset.key!, c]));
      const spy = vi.spyOn(ul, 'insertBefore');
      const append = vi.spyOn(ul, 'appendChild');
      patchRows(ul, after.map((k) => row(k)));
      expect(keysOf(ul), `${before} -> ${after}`).toEqual(after);
      for (const k of after) if (nodes.has(k)) expect(ul.querySelector(`[data-key="${k}"]`)).toBe(nodes.get(k));
      // Only new rows are inserted; nothing already placed is moved.
      const moved = [...spy.mock.calls, ...append.mock.calls].map((c) => (c[0] as HTMLElement).dataset.key);
      expect(moved.every((k) => !nodes.has(k!)), `${before} -> ${after}: moved ${moved}`).toBe(true);
      document.body.replaceChildren();
    }
  });

  it('a_focused_later_row_keeps_focus_when_an_earlier_row_leaves', () => {
    const ul = list(['a', 'b', 'c']);
    const btn = ul.querySelector<HTMLButtonElement>('[data-key="c"] button')!;
    btn.focus();
    patchRows(ul, [row('a'), row('c')]);
    expect(document.activeElement).toBe(btn);
    patchRows(ul, [row('a'), row('x'), row('c')]);
    expect(document.activeElement).toBe(btn);
  });

  it('a_rebuilt_row_hands_focus_to_the_same_control', () => {
    const ul = list(['a', 'b']);
    ul.querySelector<HTMLButtonElement>('[data-key="b"] button')!.focus();
    patchRows(ul, [row('a'), row('b', 'changed')]);
    expect(document.activeElement!.textContent).toBe('b:changed');
  });
});
