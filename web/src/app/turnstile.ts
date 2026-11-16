// Cloudflare Turnstile for room creation: the script loads only when the
// create screen opens (the one third-party request the client makes), renders
// explicitly with action `create_room`, and hands the token to createRoom.
// Tokens are single use: reset the widget after every attempt.

export const TURNSTILE_SRC = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
export const TURNSTILE_ACTION = 'create_room';
const LOAD_TIMEOUT_MS = 20_000;

export interface TurnstileApi {
  render(el: HTMLElement, opts: Record<string, unknown>): string;
  reset(id?: string): void;
  remove(id?: string): void;
}

type Win = { turnstile?: TurnstileApi };

let loading: Promise<TurnstileApi> | null = null;

/** Load the Turnstile script once; later calls share the same promise. A failed load may be retried. */
export function loadTurnstile(doc: Document = document): Promise<TurnstileApi> {
  const win = (doc.defaultView ?? globalThis) as unknown as Win;
  if (win.turnstile) return Promise.resolve(win.turnstile);
  loading ??= new Promise<TurnstileApi>((resolve, reject) => {
    const s = doc.createElement('script');
    s.src = TURNSTILE_SRC;
    s.async = true;
    s.defer = true;
    s.crossOrigin = 'anonymous';
    const timer = setTimeout(() => fail(new Error('Turnstile took too long to load')), LOAD_TIMEOUT_MS);
    const fail = (e: Error) => {
      clearTimeout(timer);
      s.remove();
      loading = null;
      reject(e);
    };
    s.onload = () => {
      clearTimeout(timer);
      if (win.turnstile) resolve(win.turnstile);
      else fail(new Error('Turnstile did not start'));
    };
    s.onerror = () => fail(new Error('Turnstile failed to load'));
    doc.head.append(s);
  });
  return loading;
}

/** Test hook: forget the shared load. */
export function resetTurnstileLoader(): void {
  loading = null;
}

export interface TurnstileWidget {
  reset(): void;
  remove(): void;
}

export function renderTurnstile(
  api: TurnstileApi,
  el: HTMLElement,
  o: { siteKey: string; onToken(t: string): void; onExpire(): void; onError(): void },
): TurnstileWidget {
  const id = api.render(el, {
    sitekey: o.siteKey,
    action: TURNSTILE_ACTION,
    theme: 'dark',
    'refresh-expired': 'auto',
    callback: (t: string) => o.onToken(t),
    'expired-callback': () => o.onExpire(),
    'timeout-callback': () => o.onExpire(),
    'error-callback': () => o.onError(),
  });
  return {
    reset: () => api.reset(id),
    remove: () => api.remove(id),
  };
}
