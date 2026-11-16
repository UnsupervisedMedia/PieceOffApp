// Tiny DOM helpers for the screens. Text always goes in as text nodes
// (never innerHTML), so names, titles and server messages cannot inject markup.

type Child = Node | string | null | undefined | false;
type Attrs = Record<string, string | number | boolean | null | undefined | EventListener>;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k.startsWith('on') && typeof v === 'function') {
      el.addEventListener(k.slice(2).toLowerCase(), v);
    } else if (k === 'class') {
      el.className = String(v);
    } else if (v === true) {
      el.setAttribute(k, '');
    } else {
      el.setAttribute(k, String(v));
    }
  }
  append(el, ...children);
  return el;
}

export function append(el: Node, ...children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    el.appendChild(typeof c === 'string' ? document.createTextNode(c) : c);
  }
}

/** A button that is never a form submit unless asked. */
export function button(label: string, onClick: () => void, attrs: Attrs = {}): HTMLButtonElement {
  const b = h('button', { type: 'button', class: 'btn', ...attrs }, label);
  b.addEventListener('click', onClick);
  return b;
}

let ids = 0;
export const uid = (prefix: string): string => `po-${prefix}-${++ids}`;

/** Move focus to the screen's first heading (or the element) for screen readers and keyboards. */
export function focusFirst(root: HTMLElement): void {
  const target =
    root.querySelector<HTMLElement>('[autofocus]') ?? root.querySelector<HTMLElement>('h1, h2') ?? root;
  if (!target.hasAttribute('tabindex') && !/^(INPUT|BUTTON|SELECT|TEXTAREA|A)$/.test(target.tagName)) {
    target.setAttribute('tabindex', '-1');
  }
  target.focus({ preventScroll: true });
}
