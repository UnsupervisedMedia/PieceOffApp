/** Small HTTP and crypto helpers shared by the Worker routes and Durable Objects. */

export const DEFAULT_SITE_ORIGIN = "https://pieceoff.app";

export function json(body: unknown, status = 200, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", ...headers },
  });
}

/** The bearer secret in `Authorization`, or "" when there is none. */
export function bearer(request: Request): string {
  const header = request.headers.get("authorization") ?? "";
  return header.startsWith("Bearer ") ? header.slice(7).trim() : "";
}

/** Exactly one origin (the site) may read cross-origin responses; never `*`. */
export function siteCors(siteOrigin: string | undefined): Record<string, string> {
  return { "access-control-allow-origin": siteOrigin || DEFAULT_SITE_ORIGIN, vary: "Origin" };
}

export function hex(buffer: ArrayBuffer): string {
  return Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, "0")).join("");
}

export async function sha256Hex(text: string): Promise<string> {
  return hex(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
}

/** `bytes` random bytes as unpadded base64url (32 bytes -> 43 characters). */
export function randomB64url(bytes = 32): string {
  const raw = crypto.getRandomValues(new Uint8Array(bytes));
  return btoa(String.fromCharCode(...raw)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Same alphabet as the axum server's invite codes (no I, O, 0, 1). */
const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
export const CODE_RE = /^[A-HJ-NP-Z2-9]{8}$/;

export function newCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return Array.from(bytes, (byte) => ALPHABET[byte % 32]).join("");
}

/** Constant-time comparison of two secrets (compares their hashes). */
export async function secretsEqual(a: string, b: string): Promise<boolean> {
  if (a.length === 0 || b.length === 0) return false;
  const [x, y] = await Promise.all([sha256Hex(a), sha256Hex(b)]);
  return crypto.subtle.timingSafeEqual(new TextEncoder().encode(x), new TextEncoder().encode(y));
}

/** A positive number from a variable, or the fallback. */
export function numVar(value: string | undefined, fallback: number): number {
  const n = Number(value);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

/** Read a small JSON body (at most `max` bytes). Empty is `{}`; anything else unreadable is null. */
export async function smallJson(request: Request, max = 4096): Promise<Record<string, unknown> | null> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > max) return null;
  const text = await request.text();
  if (text.length > max) return null;
  if (text.trim() === "") return {};
  try {
    const value = JSON.parse(text) as unknown;
    return value !== null && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}
