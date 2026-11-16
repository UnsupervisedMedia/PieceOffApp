// Keyed list patching for the room panels (board chunk only, so the lobby
// entry does not carry it).
export interface KeyedRow {
  /** Stable identity (a name, a holder id). */
  key: string;
  /** Everything the row shows: an equal signature keeps the existing node. */
  sig: string;
  build(): HTMLElement;
}

/**
 * Update `parent`'s children to `rows` in place: a row whose key and signature
 * are unchanged keeps its node (so focus, hover and screen-reader position
 * stay), rows only move when out of order, and a row that had focus and is
 * rebuilt hands it to the control with the same `data-act` in the new row.
 * Children without `data-key` (placeholders) are dropped. `extra` nodes are
 * appended after the rows (rebuilt every time; keep them free of focusables).
 */
export function patchRows(parent: HTMLElement, rows: KeyedRow[], extra: Node[] = []): void {
  const active = document.activeElement as HTMLElement | null;
  const wanted = new Set(rows.map((r) => r.key));
  // Drop stale rows (and unkeyed placeholders) first, so the rows that stay
  // are already in order and never need moving: moving a node blurs it.
  const old = new Map<string, HTMLElement>();
  for (const c of [...parent.children]) {
    const k = (c as HTMLElement).dataset.key;
    if (k !== undefined && wanted.has(k) && !old.has(k)) old.set(k, c as HTMLElement);
    else c.remove();
  }
  let refocus: { row: HTMLElement; act: string | undefined } | null = null;
  let i = 0;
  for (const r of rows) {
    let el = old.get(r.key);
    if (!el || el.dataset.sig !== r.sig) {
      const fresh = r.build();
      fresh.dataset.key = r.key;
      fresh.dataset.sig = r.sig;
      if (el && active && el.contains(active)) refocus = { row: fresh, act: active.dataset.act };
      if (el) el.replaceWith(fresh);
      el = fresh;
    }
    // Only a new row, or one genuinely out of position, is inserted.
    const at = parent.children[i] ?? null;
    if (at !== el) parent.insertBefore(el, at);
    i++;
  }
  for (const n of extra) parent.append(n);
  if (refocus) {
    const t = (refocus.act ? refocus.row.querySelector<HTMLElement>(`[data-act="${refocus.act}"]`) : null) ?? refocus.row.querySelector<HTMLElement>('button, input, a[href]');
    t?.focus();
  }
}
