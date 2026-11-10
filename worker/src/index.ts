import { UPLOADS_OFF_ERROR, builtinTitle, createRoomMedia, uploadsEnabled } from "./core.ts";
import type { Env } from "./env";
import { CODE_RE, bearer, json, newCode, sha256Hex } from "./http.ts";
import { LobbyDO } from "./lobby";
import { handleMedia } from "./media";
import {
  adminRevoke,
  adminUsage,
  approveLink,
  approvePreflight,
  clientIp,
  createLink,
  getLobby,
  info,
  pollLink,
  registry,
} from "./online.ts";
import { DEVICE_KEY_RE, RegistryDO } from "./registry";
import { RoomDO } from "./room";
import { originAllowed, trustedVerifyUrl, verifyTurnstile } from "./security";

export { LobbyDO, RegistryDO, RoomDO };

const MAX_BODY = 1_048_576;
const CODE_ATTEMPTS = 5;
const RELINK = "This computer is not linked to PieceOff Online. Link it again.";

/** Browser origins must be this site or listed; native clients send none. */
function checkOrigin(request: Request, env: Env): Response | null {
  const host = new URL(request.url).host;
  return originAllowed(request.headers.get("origin"), host, env.ALLOWED_ORIGINS ?? "")
    ? null
    : json({ error: "Origin not allowed" }, 403);
}

/**
 * Gate for room creation: per-address rate limit first (so abuse does not
 * cost a Turnstile or registry call), then either a device key (desktop
 * apps) or Turnstile (browsers). With neither a key nor a Turnstile secret
 * configured, creation is refused unless the local-development bypass is on.
 * Every admitted room also counts against the service's daily ceiling
 * (503 once it is reached); the device-key path does both in one registry call.
 * Returns the creator device's hash prefix ("" for browser rooms).
 */
async function checkCreateAllowed(
  request: Request,
  env: Env,
): Promise<{ denied: Response } | { device: string }> {
  const ip = clientIp(request);
  const { success } = await env.CREATE_LIMITER.limit({ key: ip });
  if (!success) return { denied: json({ error: "Too many rooms created. Try again later." }, 429) };
  if (request.headers.has("authorization")) {
    const key = bearer(request);
    if (!DEVICE_KEY_RE.test(key)) return { denied: json({ error: RELINK, relink: true }, 401) };
    const charged = await registry(env).chargeRoom(await sha256Hex(key));
    if (!charged.ok) {
      const body = charged.relink ? { error: charged.error, relink: true } : { error: charged.error };
      return { denied: json(body, charged.status) };
    }
    return { device: charged.device };
  }
  if (env.TURNSTILE_SECRET) {
    const ok = await verifyTurnstile({
      secret: env.TURNSTILE_SECRET,
      token: request.headers.get("x-turnstile-token") ?? "",
      ip: ip === "local" ? "" : ip,
      expectedHostname: env.TURNSTILE_HOSTNAME ?? "",
      expectedAction: "create_room",
      url: trustedVerifyUrl(env.TURNSTILE_VERIFY_URL),
    });
    if (!ok) return { denied: json({ error: "Verification failed" }, 403) };
  } else if (env.DEV_ALLOW_UNVERIFIED_CREATE !== "1") {
    return { denied: json({ error: "Not allowed" }, 403) };
  }
  const charged = await registry(env).chargeGlobalRoom();
  return charged.ok ? { device: "" } : { denied: json({ error: charged.error }, charged.status) };
}

async function createRoom(request: Request, env: Env): Promise<Response> {
  const refused = checkOrigin(request, env);
  if (refused) return refused;
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_BODY) return json({ error: "Message too large" }, 413);
  const text = await request.text();
  if (new TextEncoder().encode(text).length > MAX_BODY) {
    return json({ error: "Message too large" }, 413);
  }
  let message: { type?: unknown };
  try {
    message = JSON.parse(text);
  } catch {
    return json({ error: "Bad message" }, 400);
  }
  if (message === null || typeof message !== "object" || message.type !== "create_room") {
    return json({ error: "First message must create or join a room" }, 400);
  }
  if (
    !uploadsEnabled(env) &&
    createRoomMedia(message as Record<string, unknown>).some((sha) => builtinTitle(sha) === null)
  ) {
    return json({ error: UPLOADS_OFF_ERROR }, 403);
  }
  // Admission after the cheap checks, so a malformed body costs no room.
  const allowed = await checkCreateAllowed(request, env);
  if ("denied" in allowed) return allowed.denied;
  // The id is public (every player sees the host's id); the resume token that
  // proves it is the creator's is a secret, returned only here.
  const clientId = crypto.randomUUID();
  const roomId = crypto.randomUUID();
  // Gives back the service's daily slot, and the device's when there is one.
  const refund = async () => {
    await registry(env).refundRoom(allowed.device).catch(() => undefined);
  };
  for (let attempt = 0; attempt < CODE_ATTEMPTS; attempt++) {
    const code = newCode();
    const stub = env.ROOMS.get(env.ROOMS.idFromName(code));
    const result = await stub.init(clientId, message, roomId, code, allowed.device);
    if (result.ok) return json({ code, client_id: clientId, resume_token: result.resumeToken });
    if (result.error !== "exists") {
      await refund();
      return json({ error: result.error }, 400);
    }
  }
  await refund();
  return json({ error: "Could not pick a free room code. Please try again." }, 503);
}

async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname;
  if (path === "/health") return new Response("PieceOff rooms are running");
  if (path === "/api/info" && request.method === "GET") return info(env);
  // Emergency switch: set the secret ONLINE_PLAY_ENABLED to "0" to stop
  // all new hosted play (lobby reads, linking, creating and joining) without a
  // deploy (a secret, unlike a var, survives Git deploys). Nothing below
  // wakes a Durable Object while it is off.
  if (env.ONLINE_PLAY_ENABLED === "0") {
    if (path === "/api/lobby" && request.method === "GET") return json({ rooms: [] });
    if ((path.startsWith("/api/") && !path.startsWith("/api/admin/")) || path === "/ws") {
      return json({ error: "PieceOff Online is paused right now. Please try again later." }, 503);
    }
  }
  if (path === "/api/lobby" && request.method === "GET") return getLobby(request, env);
  if (path === "/api/rooms" && request.method === "POST") return createRoom(request, env);

  const approve = path.match(/^\/api\/links\/([^/]+)\/approve$/);
  if (approve) {
    if (request.method === "OPTIONS") return approvePreflight(env);
    if (request.method === "POST") return approveLink(request, env, approve[1]!.toUpperCase());
    return json({ error: "Not found" }, 404);
  }
  if (path === "/api/links" && request.method === "POST") {
    return checkOrigin(request, env) ?? createLink(request, env);
  }
  const link = path.match(/^\/api\/links\/([^/]+)$/);
  if (link && request.method === "GET") {
    return checkOrigin(request, env) ?? pollLink(request, env, link[1]!.toUpperCase());
  }
  if (path === "/api/admin/devices/revoke" && request.method === "POST") {
    return checkOrigin(request, env) ?? adminRevoke(request, env);
  }
  if (path === "/api/admin/usage" && request.method === "GET") {
    return checkOrigin(request, env) ?? adminUsage(request, env);
  }

  const media = path.match(/^\/api\/rooms\/([^/]+)\/media(\/.*)?$/);
  if (media) {
    const refused = checkOrigin(request, env);
    if (refused) return refused;
    const code = media[1]!.toUpperCase();
    if (!CODE_RE.test(code)) return json({ error: "Room code not found" }, 404);
    return handleMedia(request, env, code, media[2] ?? "");
  }
  if (path === "/ws") {
    const code = (url.searchParams.get("code") ?? "").trim().toUpperCase();
    if (!CODE_RE.test(code)) return json({ error: "Room code not found" }, 404);
    const refused = checkOrigin(request, env);
    if (refused) return refused;
    // Budget every code lookup before allocating or contacting its Durable Object.
    const { success } = await env.LOOKUP_LIMITER.limit({ key: clientIp(request) });
    if (!success) return json({ error: "Too many lookups. Try again later." }, 429);
    const stub = env.ROOMS.get(env.ROOMS.idFromName(code));
    return stub.fetch(request);
  }
  return new Response("Not found", { status: 404 });
}

export default {
  fetch: route,
} satisfies ExportedHandler<Env>;
