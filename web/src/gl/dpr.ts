// Device pixel ratio changes (browser zoom, moving a window to another
// monitor) do not fire `resize` on a canvas whose CSS size is unchanged; a
// `(resolution: Ndppx)` media query does, once, so it is re-armed each time.

export type MatchMedia = (query: string) => MediaQueryList;

/** The browser's matchMedia, or null where there is none (jsdom, workers). */
export function defaultMatchMedia(): MatchMedia | null {
  const g = globalThis as { matchMedia?: MatchMedia };
  return typeof g.matchMedia === 'function' ? (q) => g.matchMedia!(q) : null;
}

/** Call `onChange` whenever `devicePixelRatio` changes. Returns the unwatch. */
export function watchDpr(matchMedia: MatchMedia | null | undefined, onChange: () => void): () => void {
  if (!matchMedia) return () => {};
  let mql: MediaQueryList | null = null;
  const fire = () => {
    arm();
    onChange();
  };
  const arm = () => {
    mql?.removeEventListener('change', fire);
    mql = matchMedia(`(resolution: ${globalThis.devicePixelRatio || 1}dppx)`);
    mql.addEventListener('change', fire);
  };
  arm();
  return () => {
    mql?.removeEventListener('change', fire);
    mql = null;
  };
}
