import type { Env } from "./env";
import { handleMedia } from "./media";
import { RoomDO } from "./room";
import { originAllowed, verifyTurnstile } from "./security";

export { RoomDO };

/** Same alphabet as the axum server's invite codes (no I, O, 0, 1). */
const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_RE = /^[A-HJ-NP-Z2-9]{8}$/;
const MAX_BODY = 1_048_576;
const CODE_ATTEMPTS = 5;

function newCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return Array.from(bytes, (byte) => ALPHABET[byte % 32]).join("");
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function clientIp(request: Request): string {
  return request.headers.get("cf-connecting-ip") ?? "local";
}

/** Browser origins must be this site or listed; native clients send none. */
function checkOrigin(request: Request, env: Env): Response | null {
  const host = new URL(request.url).host;
  return originAllowed(request.headers.get("origin"), host, env.ALLOWED_ORIGINS ?? "")
    ? null
    : json({ error: "Origin not allowed" }, 403);
}

/**
 * Gate for room creation: per-address rate limit first (so abuse does not
 * cost a Turnstile call), then Turnstile. With no Turnstile secret configured
 * creation is refused, unless the local-development bypass is on.
 */
async function checkCreateAllowed(request: Request, env: Env): Promise<Response | null> {
  const ip = clientIp(request);
  const { success } = await env.CREATE_LIMITER.limit({ key: ip });
  if (!success) return json({ error: "Too many rooms created. Try again later." }, 429);
  if (env.TURNSTILE_SECRET) {
    const ok = await verifyTurnstile({
      secret: env.TURNSTILE_SECRET,
      token: request.headers.get("x-turnstile-token") ?? "",
      ip: ip === "local" ? "" : ip,
      url: env.TURNSTILE_VERIFY_URL,
    });
    return ok ? null : json({ error: "Verification failed" }, 403);
  }
  return env.DEV_ALLOW_UNVERIFIED_CREATE === "1" ? null : json({ error: "Not allowed" }, 403);
}

async function createRoom(request: Request, env: Env): Promise<Response> {
  const denied = checkOrigin(request, env) ?? (await checkCreateAllowed(request, env));
  if (denied) return denied;
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
  if (message.type !== "create_room") {
    return json({ error: "First message must create or join a room" }, 400);
  }
  // The id is public (every player sees the host's id); the resume token that
  // proves it is the creator's is a secret, returned only here.
  const clientId = crypto.randomUUID();
  const roomId = crypto.randomUUID();
  for (let attempt = 0; attempt < CODE_ATTEMPTS; attempt++) {
    const code = newCode();
    const stub = env.ROOMS.get(env.ROOMS.idFromName(code));
    const result = await stub.init(clientId, message, roomId, code);
    if (result.ok) return json({ code, client_id: clientId, resume_token: result.resumeToken });
    if (result.error !== "exists") return json({ error: result.error }, 400);
  }
  return json({ error: "Could not pick a free room code. Please try again." }, 503);
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/health") return new Response("PieceOff rooms are running");
    if (url.pathname === "/api/rooms" && request.method === "POST") {
      return createRoom(request, env);
    }
    const media = url.pathname.match(/^\/api\/rooms\/([^/]+)\/media(\/.*)?$/);
    if (media) {
      const refused = checkOrigin(request, env);
      if (refused) return refused;
      const code = media[1]!.toUpperCase();
      if (!CODE_RE.test(code)) return json({ error: "Room code not found" }, 404);
      return handleMedia(request, env, code, media[2] ?? "");
    }
    if (url.pathname === "/ws") {
      const code = (url.searchParams.get("code") ?? "").trim().toUpperCase();
      if (!CODE_RE.test(code)) return json({ error: "Room code not found" }, 404);
      const refused = checkOrigin(request, env);
      if (refused) return refused;
      const stub = env.ROOMS.get(env.ROOMS.idFromName(code));
      // Guessing codes is the only way to find a room: budget the misses.
      if (!(await stub.exists())) {
        const { success } = await env.LOOKUP_LIMITER.limit({ key: clientIp(request) });
        if (!success) return json({ error: "Too many lookups. Try again later." }, 429);
      }
      return stub.fetch(request);
    }
    return new Response("Not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
