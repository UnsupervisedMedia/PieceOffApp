import { DurableObject } from "cloudflare:workers";
import { Room, initSync } from "../wasm/pieceoff_room_wasm.js";
import wasmModule from "../wasm/pieceoff_room_wasm_bg.wasm";
import type { Env } from "./env";

initSync({ module: wasmModule });

/** Largest accepted frame, matching PIECEOFF_MAX_MESSAGE_BYTES' default. */
const MAX_FRAME = 1_048_576;
/** Longest a changed room waits before its snapshot is written to storage. */
const FLUSH_MS = 5_000;
/** An alarm is never set closer than this, so a stuck deadline cannot spin. */
const MIN_ALARM_GAP_MS = 100;
const ROOM_NOT_FOUND = "Room code not found";

interface Attachment {
  clientId: string;
  name: string;
  joined: boolean;
}

type Reply<T> = { ok: T } | { err: string };

interface InitResult {
  ok: boolean;
  error?: string;
}

/**
 * One PieceOff room. The Rust room core runs as a wasm module; this class owns
 * sockets, time, and storage. In-memory state is authoritative while the
 * object is alive and is written behind to SQLite.
 */
export class RoomDO extends DurableObject<Env> {
  private room = new Room();
  private initialized = false;
  private roster: Record<string, string> = {};
  private uploadToken: string | null = null;
  private dirtySince: number | null = null;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
    );
    const snapshot = this.readKv("snapshot");
    if (snapshot !== null) {
      try {
        this.room = Room.from_snapshot(snapshot);
        this.initialized = true;
      } catch (error) {
        console.error("stored snapshot unreadable", String(error));
      }
    }
    if (this.initialized) {
      this.roster = JSON.parse(this.readKv("roster") ?? "{}") as Record<string, string>;
      this.uploadToken = this.readKv("upload_token");
      for (const [clientId, name] of Object.entries(this.roster)) {
        this.room.restore_player(clientId, name);
      }
    }
  }

  /** Called by the Worker once, right after it mints the code. */
  async init(
    clientId: string,
    message: unknown,
    roomId: string,
    code: string,
  ): Promise<InitResult> {
    if (this.initialized) return { ok: false, error: "exists" };
    const reply = JSON.parse(
      this.room.create(clientId, JSON.stringify(message), roomId, code),
    ) as Reply<{ name: string }>;
    if ("err" in reply) {
      this.room = new Room();
      return { ok: false, error: reply.err };
    }
    this.initialized = true;
    this.roster = { [clientId]: reply.ok.name };
    this.uploadToken = crypto.randomUUID();
    this.flush();
    return { ok: true };
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return new Response("Expected a WebSocket", { status: 426 });
    }
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    if (!this.initialized) {
      server.accept();
      server.send(JSON.stringify({ type: "error", message: ROOM_NOT_FOUND }));
      server.close(1008, ROOM_NOT_FOUND);
      return new Response(null, { status: 101, webSocket: client });
    }
    const requested = new URL(request.url).searchParams.get("client");
    const live = new Set(this.attachments().map((a) => a.clientId));
    const clientId =
      requested !== null && /^[A-Za-z0-9-]{8,64}$/.test(requested) && !live.has(requested)
        ? requested
        : crypto.randomUUID();
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({ clientId, name: "", joined: false } satisfies Attachment);
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message !== "string") return this.sendError(ws, "Binary frames are not supported");
    if (message.length > MAX_FRAME) return this.sendError(ws, "Message too large");
    const attachment = ws.deserializeAttachment() as Attachment | null;
    if (attachment === null) return this.sendError(ws, "Unknown connection");
    if (!attachment.joined) return this.join(ws, attachment, message);

    const reply = JSON.parse(this.room.handle(attachment.clientId, message, Date.now())) as Reply<{
      event: unknown;
      persist: boolean;
    }>;
    if ("err" in reply) return this.sendError(ws, reply.err);
    this.broadcast(JSON.stringify(reply.ok.event));
    if (reply.ok.persist) this.markDirty();
    await this.schedule();
  }

  async webSocketClose(ws: WebSocket): Promise<void> {
    await this.drop(ws);
    try {
      ws.close(1000, "closed");
    } catch {
      // already closed
    }
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    await this.drop(ws);
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    if (this.dirtySince !== null && now >= this.dirtySince + FLUSH_MS) this.flush();
    const event = this.room.tick(now);
    if (event !== undefined) {
      this.broadcast(event);
      this.markDirty();
    }
    await this.schedule();
  }

  private async join(ws: WebSocket, attachment: Attachment, frame: string): Promise<void> {
    let parsed: { type?: unknown; name?: unknown; media_sha256?: unknown };
    try {
      parsed = JSON.parse(frame);
    } catch {
      return this.sendError(ws, "Bad message");
    }
    if (parsed.type !== "join_room") {
      return this.sendError(ws, "First message must create or join a room");
    }
    const name = typeof parsed.name === "string" ? parsed.name : "";
    const sha = typeof parsed.media_sha256 === "string" ? parsed.media_sha256 : "";
    const joined = JSON.parse(this.room.join(attachment.clientId, name, sha)) as Reply<{
      name: string;
      promoted_to_host: boolean;
      needs_upload_token: boolean;
    }>;
    if ("err" in joined) {
      this.sendError(ws, joined.err);
      ws.close(1008, "join refused");
      return;
    }
    if (joined.ok.needs_upload_token) this.setUploadToken(crypto.randomUUID());
    const welcome = this.room.welcome(attachment.clientId, joined.ok.name, this.uploadToken);
    if (welcome.startsWith('{"err"')) {
      this.room.leave(attachment.clientId, joined.ok.name);
      ws.send(this.errorFrame((JSON.parse(welcome) as { err: string }).err));
      ws.close(1008, "join refused");
      return;
    }
    attachment.name = joined.ok.name;
    attachment.joined = true;
    ws.serializeAttachment(attachment);
    this.roster[attachment.clientId] = attachment.name;
    this.writeRoster();
    ws.send(welcome);
    this.broadcast(JSON.stringify({ type: "peer_joined", name: attachment.name }), ws);
    this.markDirty();
    await this.schedule();
  }

  private async drop(ws: WebSocket): Promise<void> {
    const attachment = ws.deserializeAttachment() as Attachment | null;
    if (attachment === null || !attachment.joined) return;
    const left = JSON.parse(this.room.leave(attachment.clientId, attachment.name)) as Reply<{
      name: string;
      snapshot: unknown;
      upload_assignment: { client_id: string; sha256: string } | null;
    }>;
    delete this.roster[attachment.clientId];
    this.writeRoster();
    if ("err" in left) return;
    // Same order as the axum server: peer_left, snapshot, upload token.
    this.broadcast(JSON.stringify({ type: "peer_left", name: left.ok.name }), ws);
    if (left.ok.snapshot !== null) {
      this.broadcast(JSON.stringify({ type: "snapshot", snapshot: left.ok.snapshot }), ws);
    }
    if (left.ok.upload_assignment !== null) {
      const token = crypto.randomUUID();
      this.setUploadToken(token);
      this.broadcast(
        JSON.stringify({
          type: "media_upload_token",
          client_id: left.ok.upload_assignment.client_id,
          token,
          sha256: left.ok.upload_assignment.sha256,
        }),
        ws,
      );
    }
    this.markDirty();
    if (this.attachments(ws).every((a) => !a.joined)) this.flush();
    await this.schedule();
  }

  /** Send `eventJson` to every joined socket allowed to see it. */
  private broadcast(eventJson: string, except?: WebSocket): void {
    for (const socket of this.ctx.getWebSockets()) {
      if (socket === except) continue;
      const attachment = socket.deserializeAttachment() as Attachment | null;
      if (attachment?.joined && Room.visible_to(eventJson, attachment.clientId)) {
        try {
          socket.send(eventJson);
        } catch {
          // a dead socket is cleaned up by its close event
        }
      }
    }
  }

  private attachments(except?: WebSocket): Attachment[] {
    return this.ctx
      .getWebSockets()
      .filter((socket) => socket !== except)
      .map((socket) => socket.deserializeAttachment() as Attachment | null)
      .filter((a): a is Attachment => a !== null);
  }

  private errorFrame(message: string): string {
    return JSON.stringify({ type: "error", message });
  }

  private sendError(ws: WebSocket, message: string): void {
    try {
      ws.send(this.errorFrame(message));
    } catch {
      // socket already gone
    }
  }

  private markDirty(): void {
    if (this.dirtySince === null) this.dirtySince = Date.now();
  }

  private flush(): void {
    this.writeKv("snapshot", this.room.snapshot_json());
    this.writeRoster();
    this.dirtySince = null;
  }

  private writeRoster(): void {
    this.writeKv("roster", JSON.stringify(this.roster));
  }

  private setUploadToken(token: string): void {
    this.uploadToken = token;
    this.writeKv("upload_token", token);
  }

  /**
   * Arm one alarm for the earlier of the next storage flush and the next
   * playback deadline. A later existing alarm is moved earlier; an earlier
   * one is left to fire and re-arm itself.
   */
  private async schedule(): Promise<void> {
    const now = Date.now();
    let target: number | null = this.dirtySince === null ? null : this.dirtySince + FLUSH_MS;
    const deadline = this.room.next_tick_deadline(now);
    if (deadline >= 0) target = target === null ? deadline : Math.min(target, deadline);
    if (target === null) return;
    target = Math.max(target, now + MIN_ALARM_GAP_MS);
    const current = await this.ctx.storage.getAlarm();
    if (current === null || current <= now || target < current) {
      await this.ctx.storage.setAlarm(target);
    }
  }

  private readKv(key: string): string | null {
    const rows = this.ctx.storage.sql.exec("SELECT v FROM kv WHERE k = ?", key).toArray();
    return rows.length === 0 ? null : (rows[0]!.v as string);
  }

  private writeKv(key: string, value: string): void {
    this.ctx.storage.sql.exec(
      "INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
      key,
      value,
    );
  }
}
