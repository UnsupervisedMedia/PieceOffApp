/**
 * Routes for PieceOff Online: service info, device linking, the lobby, and
 * the admin revoke call. Room creation with a device key lives in index.ts.
 */
import { PROTOCOL_VERSION, uploadsEnabled } from "./core.ts";
import type { Env } from "./env";
import { CODE_RE, DEFAULT_SITE_ORIGIN, bearer, json, secretsEqual, sha256Hex, siteCors, smallJson } from "./http.ts";
import { sanitizeText } from "./lobby-state.ts";
import { DEVICE_KEY_RE, DEVICE_PREFIX_LENGTH } from "./registry.ts";
import { ipKey, trustedVerifyUrl, verifyTurnstile } from "./security.ts";

const DEFAULT_LINK_URL = "https://pieceoff.app/link";
const DEFAULT_SHARE_URL = "https://pieceoff.app/join";
const POLL_SECRET_RE = /^[A-Za-z0-9_-]{43}$/;
const MAX_DEVICE_NAME = 40;

export const registry = (env: Env) => env.REGISTRY.get(env.REGISTRY.idFromName("registry"));
export const lobby = (env: Env) => env.LOBBY.get(env.LOBBY.idFromName("lobby"));

export function clientIp(request: Request): string {
  return ipKey(request.headers.get("cf-connecting-ip") ?? "local");
}

const siteOrigin = (env: Env): string => env.SITE_ORIGIN || DEFAULT_SITE_ORIGIN;

/** `GET /api/info`: lets a client tell this hosted service from a self-hosted server. */
export function info(env: Env): Response {
  return json(
    {
      service: "pieceoff-rooms",
      protocol: PROTOCOL_VERSION,
      hosted: true,
      link_url: env.LINK_URL || DEFAULT_LINK_URL,
      share_url: env.SHARE_URL || DEFAULT_SHARE_URL,
      features: ["device_keys", "lobby"],
      uploads: uploadsEnabled(env),
    },
    200,
    siteCors(siteOrigin(env)),
  );
}

/** `POST /api/links`: a desktop app asks for a link code. */
export async function createLink(request: Request, env: Env): Promise<Response> {
  const { success } = await env.LINK_LIMITER.limit({ key: clientIp(request) });
  if (!success) return json({ error: "Too many link requests. Try again in a minute." }, 429);
  const body = await smallJson(request);
  if (body === null) return json({ error: "Bad message" }, 400);
  if (body.device_name !== undefined && typeof body.device_name !== "string") {
    return json({ error: "device_name must be a string" }, 400);
  }
  const name = sanitizeText(body.device_name ?? "", MAX_DEVICE_NAME);
  const made = await registry(env).createLink(name);
  if (!made.ok) return json({ error: made.error }, 503);
  return json({
    link_code: made.code,
    poll_secret: made.pollSecret,
    url: `${env.LINK_URL || DEFAULT_LINK_URL}?c=${made.code}`,
    expires_in: made.expiresIn,
  });
}

/** Answer the browser's preflight for the approve call. */
export function approvePreflight(env: Env): Response {
  return new Response(null, {
    status: 204,
    headers: {
      ...siteCors(siteOrigin(env)),
      "access-control-allow-methods": "POST",
      "access-control-allow-headers": "x-turnstile-token, content-type",
      "access-control-max-age": "600",
    },
  });
}

/**
 * `POST /api/links/<code>/approve`: browser only, from the site's own page,
 * with a Turnstile token for action `link_device`.
 */
export async function approveLink(request: Request, env: Env, code: string): Promise<Response> {
  const cors = siteCors(siteOrigin(env));
  if (request.headers.get("origin") !== siteOrigin(env)) return json({ error: "Origin not allowed" }, 403, cors);
  const ip = clientIp(request);
  const { success } = await env.LINK_LIMITER.limit({ key: `approve:${ip}` });
  if (!success) return json({ error: "Too many attempts. Try again in a minute." }, 429, cors);
  if (env.TURNSTILE_SECRET) {
    const ok = await verifyTurnstile({
      secret: env.TURNSTILE_SECRET,
      token: request.headers.get("x-turnstile-token") ?? "",
      ip: ip === "local" ? "" : ip,
      expectedHostname: env.TURNSTILE_HOSTNAME ?? "",
      expectedAction: "link_device",
      url: trustedVerifyUrl(env.TURNSTILE_VERIFY_URL),
    });
    if (!ok) return json({ error: "Verification failed" }, 403, cors);
  } else if (env.DEV_ALLOW_UNVERIFIED_CREATE !== "1") {
    return json({ error: "Not allowed" }, 403, cors);
  }
  if (!CODE_RE.test(code)) return json({ error: "Link code not found or expired" }, 404, cors);
  const result = await registry(env).approveLink(code);
  if (result === "missing") return json({ error: "Link code not found or expired" }, 404, cors);
  if (result === "already") return json({ error: "This computer is already linked" }, 409, cors);
  return json({ ok: true }, 200, cors);
}

/** `GET /api/links/<code>`: the app polls with its poll secret. */
export async function pollLink(request: Request, env: Env, code: string): Promise<Response> {
  const { success } = await env.LOBBY_LIMITER.limit({ key: `poll:${clientIp(request)}` });
  if (!success) return json({ error: "Polling too fast. Slow down." }, 429);
  const secret = bearer(request);
  if (!CODE_RE.test(code) || !POLL_SECRET_RE.test(secret)) return json({ error: "Link code not found or expired" }, 404);
  const polled = await registry(env).pollLink(code, secret);
  if (polled.status === "missing") return json({ error: "Link code not found or expired" }, 404);
  if (polled.status === "pending") return json({ status: "pending" }, 202);
  return json({ status: "approved", device_key: polled.deviceKey });
}

/** `GET /api/lobby`: listed rooms, cached briefly at the edge. */
export async function getLobby(request: Request, env: Env): Promise<Response> {
  const { success } = await env.LOBBY_LIMITER.limit({ key: clientIp(request) });
  const cors = siteCors(siteOrigin(env));
  if (!success) return json({ error: "Too many lobby requests. Try again in a minute." }, 429, cors);
  const cacheSeconds = Number(env.LOBBY_CACHE_SECONDS ?? "5");
  const cacheKey = new Request(new URL("/api/lobby", request.url).toString(), { method: "GET" });
  const cache = cacheSeconds > 0 ? await caches.open("lobby").catch(() => null) : null;
  if (cache) {
    const hit = await cache.match(cacheKey).catch(() => undefined);
    if (hit) return hit;
  }
  const body = JSON.stringify({ rooms: await lobby(env).list() });
  const headers = (maxAge: number) => ({
    "content-type": "application/json",
    "cache-control": `public, max-age=${maxAge}`,
    ...cors,
  });
  if (cache) {
    await cache.put(cacheKey, new Response(body, { headers: headers(cacheSeconds) })).catch(() => undefined);
  }
  return new Response(body, { headers: headers(5) });
}

/**
 * `POST /api/admin/devices/revoke` `{"device_prefix": "<16 hex>"}` or
 * `{"device_key": "pok_..."}`, optional `"revoked": false` to restore.
 * `Authorization: Bearer <ADMIN_TOKEN>`. Not found without the secret set.
 */
export async function adminRevoke(request: Request, env: Env): Promise<Response> {
  if (!env.ADMIN_TOKEN) return new Response("Not found", { status: 404 });
  const { success } = await env.LINK_LIMITER.limit({ key: `admin:${clientIp(request)}` });
  if (!success) return json({ error: "Too many requests" }, 429);
  if (!(await secretsEqual(bearer(request), env.ADMIN_TOKEN))) return json({ error: "Not allowed" }, 403);
  const body = await smallJson(request);
  if (body === null) return json({ error: "Bad message" }, 400);
  const revoked = body.revoked !== false;
  let match: { prefix?: string; keyHash?: string };
  if (typeof body.device_key === "string" && DEVICE_KEY_RE.test(body.device_key)) {
    match = { keyHash: await sha256Hex(body.device_key) };
  } else if (
    typeof body.device_prefix === "string" &&
    new RegExp(`^[0-9a-f]{${DEVICE_PREFIX_LENGTH}}$`).test(body.device_prefix)
  ) {
    match = { prefix: body.device_prefix };
  } else {
    return json({ error: "Give device_key or a 16-character device_prefix" }, 400);
  }
  const changed = await registry(env).setRevoked(match, revoked);
  if (changed === 0) return json({ error: "No such device" }, 404);
  return json({ ok: true, revoked, devices: changed });
}
