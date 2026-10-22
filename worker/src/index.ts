import type { Env } from "./env";
import { RoomDO } from "./room";

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

/**
 * Gate for room creation. Plan 1B-2 replaces this with Turnstile verification
 * and a per-IP rate limit; until then creation is open on local dev only.
 */
async function verifyCreateAllowed(_request: Request, _env: Env): Promise<boolean> {
  return true;
}

async function createRoom(request: Request, env: Env): Promise<Response> {
  if (!(await verifyCreateAllowed(request, env))) return json({ error: "Not allowed" }, 403);
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
    if (url.pathname === "/ws") {
      const code = (url.searchParams.get("code") ?? "").trim().toUpperCase();
      if (!CODE_RE.test(code)) return json({ error: "Room code not found" }, 404);
      return env.ROOMS.get(env.ROOMS.idFromName(code)).fetch(request);
    }
    return new Response("Not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
