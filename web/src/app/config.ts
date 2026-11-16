// Site configuration, read once from `window.PIECEOFF_WEB_CONFIG` (ruling R4)
// so the release bundle is site-agnostic. The page that mounts the client sets
// it before loading play.js, for example:
//
//   <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
//   <script>window.PIECEOFF_WEB_CONFIG = { api: "https://rooms.pieceoff.app",
//     turnstileSiteKey: "0x4AAAAAAF7xNWih_NrGxdF_", assetBase: "/play-app/" };</script>
//
// `viewport-fit=cover` is needed for the safe-area insets the stylesheet uses.

export interface WebConfig {
  /** Rooms Worker origin, no trailing slash. */
  api: string;
  /** Public Turnstile site key (widget `pieceoff-online`). */
  turnstileSiteKey: string;
  /** URL prefix the release bundle is served from (wasm, builtin/), with a trailing slash. */
  assetBase: string;
}

export const DEFAULT_CONFIG: Readonly<WebConfig> = Object.freeze({
  api: 'https://rooms.pieceoff.app',
  turnstileSiteKey: '0x4AAAAAAF7xNWih_NrGxdF_',
  assetBase: '/play-app/',
});

const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]']);

function cleanApi(v: unknown): string | null {
  if (typeof v !== 'string' || v.length > 200) return null;
  let u: URL;
  try {
    u = new URL(v);
  } catch {
    return null;
  }
  const ok = u.protocol === 'https:' || (u.protocol === 'http:' && LOCAL_HOSTS.has(u.hostname));
  if (!ok || u.username || u.password || u.search || u.hash) return null;
  return u.origin + u.pathname.replace(/\/+$/, '');
}

function cleanKey(v: unknown): string | null {
  return typeof v === 'string' && /^[\w-]{1,100}$/.test(v) ? v : null;
}

function cleanBase(v: unknown): string | null {
  if (typeof v !== 'string' || v === '' || v.length > 200 || /[\s"'<>\\]/.test(v)) return null;
  // A path on this site or an https origin; never javascript: and friends.
  if (!v.startsWith('/') && !v.startsWith('https://')) return null;
  if (v.startsWith('//')) return null;
  return v.endsWith('/') ? v : `${v}/`;
}

export function readConfig(win: { PIECEOFF_WEB_CONFIG?: unknown } = globalThis as never): WebConfig {
  const raw = win.PIECEOFF_WEB_CONFIG;
  const o = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {};
  return {
    api: cleanApi(o.api) ?? DEFAULT_CONFIG.api,
    turnstileSiteKey: cleanKey(o.turnstileSiteKey) ?? DEFAULT_CONFIG.turnstileSiteKey,
    assetBase: cleanBase(o.assetBase) ?? DEFAULT_CONFIG.assetBase,
  };
}

/** `wss://host/ws?code=CODE` for an `https://host` API (ws:// for local http). */
export function wsUrl(api: string, code: string): string {
  const u = new URL(api);
  u.protocol = u.protocol === 'http:' ? 'ws:' : 'wss:';
  u.pathname = `${u.pathname.replace(/\/+$/, '')}/ws`;
  u.search = `?code=${encodeURIComponent(code)}`;
  return u.toString();
}
