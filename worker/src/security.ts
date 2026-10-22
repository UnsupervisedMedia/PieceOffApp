/**
 * Request admission helpers: which browser origins may talk to the worker and
 * whether a Turnstile token is genuine. Pure of Cloudflare bindings so they
 * can be tested directly.
 */

/**
 * Browsers always send `Origin` on WebSocket upgrades and cross-site fetches;
 * native clients send none and are allowed. A browser origin must be the
 * site's own host or appear exactly in `allowedList` (comma separated).
 */
export function originAllowed(
  origin: string | null,
  requestHost: string,
  allowedList: string,
): boolean {
  if (origin === null) return true;
  let parsed: URL;
  try {
    parsed = new URL(origin);
  } catch {
    return false;
  }
  if (parsed.origin === "null" || (parsed.protocol !== "https:" && parsed.protocol !== "http:")) {
    return false;
  }
  if (parsed.host === requestHost) return true;
  return allowedList
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0)
    .includes(parsed.origin);
}

export const TURNSTILE_VERIFY_URL = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const VERIFY_TIMEOUT_MS = 5_000;

/**
 * Ask Turnstile whether `token` is valid. Anything but an explicit
 * `success: true` from a 200 response is a denial: a missing token, an
 * unreachable or erroring service, or a malformed answer all fail closed.
 */
export async function verifyTurnstile(options: {
  secret: string;
  token: string;
  ip: string;
  url?: string;
  fetcher?: typeof fetch;
}): Promise<boolean> {
  if (options.token.length === 0 || options.secret.length === 0) return false;
  const form = new URLSearchParams({ secret: options.secret, response: options.token });
  if (options.ip.length > 0) form.set("remoteip", options.ip);
  try {
    const response = await (options.fetcher ?? fetch)(options.url ?? TURNSTILE_VERIFY_URL, {
      method: "POST",
      body: form,
      signal: AbortSignal.timeout(VERIFY_TIMEOUT_MS),
    });
    if (response.status !== 200) return false;
    const result = (await response.json()) as { success?: unknown };
    return result.success === true;
  } catch {
    return false;
  }
}

/**
 * Rate-limit and socket-cap key for a client address. IPv4 is used as is; an
 * IPv6 client typically controls a whole /64, so those share one key.
 */
export function ipKey(ip: string): string {
  if (!ip.includes(":")) return ip;
  const [head = "", tail = ""] = ip.toLowerCase().split("::");
  const groups = head === "" ? [] : head.split(":");
  const rest = tail === "" ? [] : tail.split(":");
  const missing = 8 - groups.length - rest.length;
  const full = ip.includes("::") ? [...groups, ...Array(Math.max(0, missing)).fill("0"), ...rest] : groups;
  const first4 = full.slice(0, 4).map((g) => (g === "" ? "0" : g.replace(/^0+(?=.)/, "")));
  while (first4.length < 4) first4.push("0");
  return `${first4.join(":")}::/64`;
}

/**
 * Only a loopback address may replace Cloudflare's siteverify endpoint (the
 * tests point it at a local stand-in). Anything else is ignored, so a stray
 * setting can never send the Turnstile secret to another host.
 */
export function trustedVerifyUrl(url: string | undefined): string | undefined {
  if (url === undefined) return undefined;
  try {
    const host = new URL(url).hostname;
    return host === "127.0.0.1" || host === "localhost" ? url : undefined;
  } catch {
    return undefined;
  }
}
