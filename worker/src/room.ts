import { DurableObject } from "cloudflare:workers";
import { Room, initSync } from "../wasm/pieceoff_room_wasm.js";
import wasmModule from "../wasm/pieceoff_room_wasm_bg.wasm";
import type { Env } from "./env";
import {
  type Failure,
  type Limits,
  type PartRecord,
  type Upload,
  checkPart,
  missingParts,
  planUpload,
  recordPart,
} from "./media-state.ts";
import { alarmTarget, shouldSetAlarm } from "./schedule.ts";

initSync({ module: wasmModule });

/** Largest accepted frame in bytes, matching PIECEOFF_MAX_MESSAGE_BYTES' default. */
const MAX_FRAME_BYTES = 1_048_576;
const DEFAULT_FLUSH_MS = 5_000;
const DEFAULT_IDLE_SOCKET_MS = 90_000;
/** Clients ping about this often; the sweep runs at most this far apart. */
const MAX_SWEEP_MS = 20_000;
/** An alarm is never set closer than this, so a stuck deadline cannot spin. */
const MIN_ALARM_GAP_MS = 100;
/** setAlarm has a limited date range; wake early rather than ask for more. */
const MAX_ALARM_AHEAD_MS = 7 * 24 * 3_600_000;
/** Tick catch-up after a quiet spell is bounded so a bad snapshot cannot loop. */
const MAX_CATCH_UP_TICKS = 100;
const ROOM_NOT_FOUND = "Room code not found";
const MIB = 1024 * 1024;
const DEFAULT_ROOM_QUOTA_BYTES = 1024 * MIB;
const MAX_R2_PARTS = 10_000;
/** Resume secrets kept for players who have left (their identity on return). */
const MAX_KEPT_SECRETS = 64;

/** What a media call returns over RPC: data on success, a status on failure. */
export type MediaReply<T> = ({ ok: true } & T) | (Failure & { missing?: number[] });

export interface UploadInfo {
  uploadId: string;
  partCount: number;
  partSize: number;
  missing: number[];
}

interface Attachment {
  /** The player's public id. Replaced when the player presents a resume token. */
  clientId: string;
  name: string;
  joined: boolean;
  joinedAt: number;
}

type Reply<T> = { ok: T } | { err: string };

interface InitResult {
  ok: boolean;
  error?: string;
  resumeToken?: string;
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

function utf8Length(text: string): number {
  // UTF-8 is at most 3 bytes per UTF-16 unit, so short strings need no encoding.
  return text.length * 3 <= MAX_FRAME_BYTES ? text.length * 3 : new TextEncoder().encode(text).length;
}

/**
 * One PieceOff room. The Rust room core runs as a wasm module; this class owns
 * sockets, time, and storage. In-memory state is authoritative while the
 * object is alive and is written behind to SQLite.
 *
 * Identity: a player's public id is broadcast to everyone (it appears in
 * `host_id` and `held_by`), so it grants nothing. Reconnecting as the same
 * player, including the creator claiming the host role, requires the secret
 * resume token the server minted; only its hash is stored.
 *
 * Durability: the snapshot is flushed within `FLUSH_MS` of a change and when
 * the last player leaves. The roster, secrets and upload token are written
 * immediately. Hibernation waits for about 10 s of quiet, longer than the
 * flush delay, so a normal sleep loses nothing; a hard eviction can lose up to
 * `FLUSH_MS` of piece movement.
 */
export class RoomDO extends DurableObject<Env> {
  private room = new Room();
  private initialized = false;
  /** Joined players, plus the creator until they first connect. */
  private roster: Record<string, string> = {};
  /** sha256(resume token) -> public client id. */
  private secrets: Record<string, string> = {};
  /** The creator's id until they first join. */
  private pending: string | null = null;
  private uploadToken: string | null = null;
  private dirtySince: number | null = null;
  private lastSeen = new Map<string, number>();
  private readonly flushMs: number;
  private readonly idleSocketMs: number;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.flushMs = Number(env.FLUSH_MS) > 0 ? Number(env.FLUSH_MS) : DEFAULT_FLUSH_MS;
    this.idleSocketMs =
      Number(env.IDLE_SOCKET_MS) > 0 ? Number(env.IDLE_SOCKET_MS) : DEFAULT_IDLE_SOCKET_MS;
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    this.ctx.blockConcurrencyWhile(async () => this.restore());
  }

  /** Load from storage, then drop anyone who is no longer actually connected. */
  private restore(): void {
    // Unknown room codes also construct a Durable Object; never write for them.
    const hasTable =
      this.ctx.storage.sql
        .exec("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kv'")
        .toArray().length > 0;
    if (!hasTable) return;
    const snapshot = this.readKv("snapshot");
    if (snapshot === null) return;
    try {
      this.room = Room.from_snapshot(snapshot);
    } catch (error) {
      console.error("stored snapshot unreadable", String(error));
      return;
    }
    this.initialized = true;
    this.roster = JSON.parse(this.readKv("roster") ?? "{}") as Record<string, string>;
    this.secrets = JSON.parse(this.readKv("secrets") ?? "{}") as Record<string, string>;
    this.pending = this.readKv("pending");
    this.uploadToken = this.readKv("upload_token");
    for (const [clientId, name] of Object.entries(this.roster)) {
      this.wasm(() => this.room.restore_player(clientId, name));
    }

    // Connections that survived are in the attachments; everyone else in the
    // roster, and anyone still holding a piece, is gone (a deploy or reset
    // closes sockets without a close event). Release them like a normal leave.
    const live = new Set(
      this.attachments()
        .filter((a) => a.joined)
        .map((a) => a.clientId),
    );
    const gone = new Set<string>();
    for (const clientId of Object.keys(this.roster)) {
      if (!live.has(clientId) && clientId !== this.pending) gone.add(clientId);
    }
    const stored = JSON.parse(this.wasm(() => this.room.snapshot_json())) as {
      pieces: { held_by: string | null }[];
    } | null;
    for (const piece of stored?.pieces ?? []) {
      if (piece.held_by !== null && !live.has(piece.held_by) && piece.held_by !== this.pending) {
        gone.add(piece.held_by);
      }
    }
    for (const clientId of gone) this.releasePlayer(clientId, this.roster[clientId] ?? "");
    if (gone.size > 0) {
      this.writeRoster();
      this.writeSecrets();
      this.flush();
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
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
    );
    const reply = JSON.parse(
      this.wasm(() => this.room.create(clientId, JSON.stringify(message), roomId, code)),
    ) as Reply<{ name: string }>;
    if ("err" in reply) {
      this.room = new Room();
      return { ok: false, error: reply.err };
    }
    this.initialized = true;
    this.roster = { [clientId]: reply.ok.name };
    this.pending = clientId;
    const resumeToken = crypto.randomUUID();
    this.secrets = { [await sha256Hex(resumeToken)]: clientId };
    this.uploadToken = crypto.randomUUID();
    this.writeKv("pending", clientId);
    this.writeKv("room_id", roomId);
    this.writeKv("upload_token", this.uploadToken);
    this.writeSecrets();
    this.flush();
    return { ok: true, resumeToken };
  }

  // ---- Media (resumable multipart uploads into R2) -------------------------
  // The Worker streams part bytes; this object owns who may upload, quota,
  // part bookkeeping and the room's media list. The upload token and R2
  // upload ids never leave the Worker/Durable Object pair.

  private mediaLimits(): Limits {
    const num = (value: string | undefined, fallback: number): number =>
      Number(value) > 0 ? Number(value) : fallback;
    const quotaBytes = num(this.env.ROOM_MEDIA_QUOTA_BYTES, DEFAULT_ROOM_QUOTA_BYTES);
    return {
      quotaBytes,
      maxFileBytes: num(this.env.MAX_MEDIA_BYTES, quotaBytes),
      minPartBytes: num(this.env.MEDIA_MIN_PART_BYTES, 5 * MIB),
      maxPartBytes: num(this.env.MEDIA_MAX_PART_BYTES, 64 * MIB),
      maxParts: MAX_R2_PARTS,
    };
  }

  private ensureMediaTables(): void {
    this.ctx.storage.sql.exec(
      `CREATE TABLE IF NOT EXISTS uploads (
         id TEXT PRIMARY KEY, sha256 TEXT NOT NULL UNIQUE, size INTEGER NOT NULL,
         part_size INTEGER NOT NULL, part_count INTEGER NOT NULL,
         r2_upload_id TEXT NOT NULL, parts TEXT NOT NULL)`,
    );
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS media (sha256 TEXT PRIMARY KEY, size INTEGER NOT NULL, key TEXT NOT NULL)",
    );
  }

  private mediaKey(sha256: string): string {
    return `room/${this.readKv("room_id") ?? "legacy"}/${sha256}`;
  }

  /** Constant-time check of a presented bearer secret against the upload token. */
  private async isUploadToken(presented: string): Promise<boolean> {
    if (this.uploadToken === null || presented.length === 0) return false;
    const [a, b] = await Promise.all([sha256Hex(presented), sha256Hex(this.uploadToken)]);
    return crypto.subtle.timingSafeEqual(new TextEncoder().encode(a), new TextEncoder().encode(b));
  }

  private loadUpload(id: string): (Upload & { r2UploadId: string }) | null {
    const rows = this.ctx.storage.sql.exec("SELECT * FROM uploads WHERE id = ?", id).toArray();
    return rows.length === 0 ? null : this.toUpload(rows[0]!);
  }

  private toUpload(row: Record<string, SqlStorageValue>): Upload & { r2UploadId: string } {
    return {
      id: row.id as string,
      sha256: row.sha256 as string,
      size: row.size as number,
      partSize: row.part_size as number,
      partCount: row.part_count as number,
      r2UploadId: row.r2_upload_id as string,
      parts: JSON.parse(row.parts as string) as Record<string, PartRecord>,
    };
  }

  private uploadInfo(upload: Upload): UploadInfo {
    return {
      uploadId: upload.id,
      partCount: upload.partCount,
      partSize: upload.partSize,
      missing: missingParts(upload),
    };
  }

  private mediaUsage(): { completedBytes: number; reservedBytes: number } {
    const sum = (table: string): number =>
      Number(
        this.ctx.storage.sql.exec(`SELECT COALESCE(SUM(size), 0) AS n FROM ${table}`).one().n,
      );
    return { completedBytes: sum("media"), reservedBytes: sum("uploads") };
  }

  private async authorizeUpload(token: string): Promise<Failure | null> {
    if (!this.initialized) return { ok: false, status: 404, error: ROOM_NOT_FOUND };
    if (!(await this.isUploadToken(token))) return { ok: false, status: 403, error: "Not allowed" };
    this.ensureMediaTables();
    return null;
  }

  async startUpload(
    token: string,
    request: { sha256: string; size: number; partSize: number },
  ): Promise<MediaReply<{ complete: boolean } & Partial<UploadInfo>>> {
    const denied = await this.authorizeUpload(token);
    if (denied) return denied;
    const sha256 = String(request.sha256).toLowerCase();
    if (this.ctx.storage.sql.exec("SELECT 1 FROM media WHERE sha256 = ?", sha256).toArray().length) {
      return { ok: true, complete: true };
    }
    const existing = this.ctx.storage.sql
      .exec("SELECT * FROM uploads WHERE sha256 = ?", sha256)
      .toArray();
    if (existing.length > 0) {
      const upload = this.toUpload(existing[0]!);
      if (upload.size !== request.size || upload.partSize !== request.partSize) {
        return { ok: false, status: 409, error: "An upload with different sizes is in progress" };
      }
      return { ok: true, complete: false, ...this.uploadInfo(upload) };
    }
    const plan = planUpload({ ...request, sha256 }, this.mediaLimits(), this.mediaUsage());
    if (!plan.ok) return plan;
    // Reserve before awaiting R2 so concurrent starts see this upload's bytes.
    const uploadId = crypto.randomUUID();
    this.ctx.storage.sql.exec(
      "INSERT INTO uploads (id, sha256, size, part_size, part_count, r2_upload_id, parts) VALUES (?, ?, ?, ?, ?, '', '{}')",
      uploadId,
      sha256,
      request.size,
      request.partSize,
      plan.partCount,
    );
    try {
      const multipart = await this.env.MEDIA.createMultipartUpload(this.mediaKey(sha256));
      this.ctx.storage.sql.exec(
        "UPDATE uploads SET r2_upload_id = ? WHERE id = ?",
        multipart.uploadId,
        uploadId,
      );
    } catch (error) {
      this.ctx.storage.sql.exec("DELETE FROM uploads WHERE id = ?", uploadId);
      console.error("createMultipartUpload failed", String(error));
      return { ok: false, status: 502, error: "Media storage is unavailable" };
    }
    return { ok: true, complete: false, ...this.uploadInfo(this.loadUpload(uploadId)!) };
  }

  async uploadStatus(token: string, uploadId: string): Promise<MediaReply<UploadInfo>> {
    const denied = await this.authorizeUpload(token);
    if (denied) return denied;
    const upload = this.loadUpload(uploadId);
    if (upload === null) return { ok: false, status: 404, error: "No such upload" };
    return { ok: true, ...this.uploadInfo(upload) };
  }

  /** Check a part before its bytes are streamed; returns where to send them. */
  async preparePart(
    token: string,
    uploadId: string,
    n: number,
    size: number,
  ): Promise<MediaReply<{ key: string; r2UploadId: string }>> {
    const denied = await this.authorizeUpload(token);
    if (denied) return denied;
    const upload = this.loadUpload(uploadId);
    if (upload === null || upload.r2UploadId === "") {
      return { ok: false, status: 404, error: "No such upload" };
    }
    const checked = checkPart(upload, n, size);
    if (!checked.ok) return checked;
    return { ok: true, key: this.mediaKey(upload.sha256), r2UploadId: upload.r2UploadId };
  }

  async recordUploadedPart(
    token: string,
    uploadId: string,
    n: number,
    part: PartRecord,
  ): Promise<MediaReply<{ missing: number[] }>> {
    const denied = await this.authorizeUpload(token);
    if (denied) return denied;
    const upload = this.loadUpload(uploadId);
    if (upload === null) return { ok: false, status: 404, error: "No such upload" };
    const checked = checkPart(upload, n, part.size);
    if (!checked.ok) return checked;
    const next = recordPart(upload, n, part);
    this.ctx.storage.sql.exec(
      "UPDATE uploads SET parts = ? WHERE id = ?",
      JSON.stringify(next.parts),
      uploadId,
    );
    return { ok: true, missing: missingParts(next) };
  }

  async completeUpload(token: string, uploadId: string): Promise<MediaReply<object>> {
    const denied = await this.authorizeUpload(token);
    if (denied) return denied;
    const upload = this.loadUpload(uploadId);
    if (upload === null) return { ok: false, status: 404, error: "No such upload" };
    const missing = missingParts(upload);
    if (missing.length > 0) return { ok: false, status: 409, error: "Parts are missing", missing };
    const key = this.mediaKey(upload.sha256);
    try {
      const parts = Object.entries(upload.parts)
        .map(([n, part]) => ({ partNumber: Number(n), etag: part.etag }))
        .sort((a, b) => a.partNumber - b.partNumber);
      await this.env.MEDIA.resumeMultipartUpload(key, upload.r2UploadId).complete(parts);
    } catch (error) {
      console.error("completing the multipart upload failed", String(error));
      return { ok: false, status: 502, error: "Media storage rejected the upload" };
    }
    this.ctx.storage.sql.exec(
      "INSERT INTO media (sha256, size, key) VALUES (?, ?, ?) ON CONFLICT (sha256) DO NOTHING",
      upload.sha256,
      upload.size,
      key,
    );
    this.ctx.storage.sql.exec("DELETE FROM uploads WHERE id = ?", uploadId);
    const event = this.wasm(() => this.room.mark_item_available(upload.sha256));
    if (!event.startsWith('{"err"')) {
      this.broadcast(event);
      this.markDirty();
      await this.schedule();
    }
    return { ok: true };
  }

  /** A joined or pending player may read this room's finished media. */
  async authorizeDownload(
    resumeToken: string,
    sha256: string,
  ): Promise<MediaReply<{ key: string }>> {
    if (!this.initialized) return { ok: false, status: 404, error: ROOM_NOT_FOUND };
    if (resumeToken.length === 0 || this.secrets[await sha256Hex(resumeToken)] === undefined) {
      return { ok: false, status: 403, error: "Not allowed" };
    }
    this.ensureMediaTables();
    const rows = this.ctx.storage.sql
      .exec("SELECT key FROM media WHERE sha256 = ?", sha256.toLowerCase())
      .toArray();
    if (rows.length === 0) return { ok: false, status: 404, error: "No such media" };
    return { ok: true, key: rows[0]!.key as string };
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
    this.ctx.acceptWebSocket(server);
    // The real id is assigned on join; until then this one is never trusted.
    server.serializeAttachment({
      clientId: crypto.randomUUID(),
      name: "",
      joined: false,
      joinedAt: Date.now(),
    } satisfies Attachment);
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message !== "string") return this.sendError(ws, "Binary frames are not supported");
    if (utf8Length(message) > MAX_FRAME_BYTES) return this.sendError(ws, "Message too large");
    const attachment = ws.deserializeAttachment() as Attachment | null;
    if (attachment === null) return this.sendError(ws, "Unknown connection");
    this.lastSeen.set(attachment.clientId, Date.now());
    if (!attachment.joined) return this.join(ws, attachment, message);

    const reply = JSON.parse(
      this.wasm(() => this.room.handle(attachment.clientId, message, Date.now())),
    ) as Reply<{ event: unknown; persist: boolean }>;
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
    await this.sweepSilentSockets(now);
    if (this.dirtySince !== null && now >= this.dirtySince + this.flushMs) this.flush();
    // Only advance playback while someone is watching; join catches up.
    if (this.joinedCount() > 0) {
      const event = this.wasm(() => this.room.tick(now));
      if (event !== undefined) {
        this.broadcast(event);
        this.markDirty();
      }
    }
    await this.schedule();
  }

  /** Drop players whose socket has said nothing (not even a ping) for too long. */
  private async sweepSilentSockets(now: number): Promise<void> {
    for (const socket of this.ctx.getWebSockets()) {
      const attachment = socket.deserializeAttachment() as Attachment | null;
      if (attachment === null || !attachment.joined) continue;
      const auto = this.ctx.getWebSocketAutoResponseTimestamp(socket)?.getTime() ?? 0;
      const last = Math.max(this.lastSeen.get(attachment.clientId) ?? 0, auto, attachment.joinedAt);
      if (now - last > this.idleSocketMs) {
        await this.drop(socket);
        try {
          socket.close(1001, "idle");
        } catch {
          // already closed
        }
      }
    }
  }

  private async join(ws: WebSocket, attachment: Attachment, frame: string): Promise<void> {
    let parsed: { type?: unknown; name?: unknown; media_sha256?: unknown; resume_token?: unknown };
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

    // Identity: a valid, unused resume token adopts its player; anything else
    // is a new player with a new secret.
    let clientId = crypto.randomUUID();
    let resumeToken: string | null = null;
    let minted = false;
    if (typeof parsed.resume_token === "string" && parsed.resume_token.length > 0) {
      const known = this.secrets[await sha256Hex(parsed.resume_token)];
      if (known !== undefined) {
        const inUse = this.attachments(ws).some((a) => a.joined && a.clientId === known);
        if (inUse) {
          this.sendError(ws, "That player is already connected");
          ws.close(1008, "already connected");
          return;
        }
        clientId = known;
        resumeToken = parsed.resume_token;
      }
    }
    if (resumeToken === null) {
      resumeToken = crypto.randomUUID();
      this.secrets[await sha256Hex(resumeToken)] = clientId;
      this.pruneSecrets();
      minted = true;
    }

    // A room left alone keeps nominally playing; catch up before anyone looks.
    if (this.joinedCount() === 0) {
      for (let i = 0; i < MAX_CATCH_UP_TICKS; i++) {
        if (this.wasm(() => this.room.tick(Date.now())) === undefined) break;
        this.markDirty();
      }
    }

    const joined = JSON.parse(this.wasm(() => this.room.join(clientId, name, sha))) as Reply<{
      name: string;
      promoted_to_host: boolean;
      needs_upload_token: boolean;
    }>;
    if ("err" in joined) {
      if (minted) delete this.secrets[await sha256Hex(resumeToken)];
      this.sendError(ws, joined.err);
      ws.close(1008, "join refused");
      return;
    }
    if (joined.ok.needs_upload_token) this.setUploadToken(crypto.randomUUID());
    const welcome = this.wasm(() =>
      this.room.welcome(clientId, joined.ok.name, this.uploadToken),
    );
    if (welcome.startsWith('{"err"')) {
      this.wasm(() => this.room.leave(clientId, joined.ok.name));
      if (minted) delete this.secrets[await sha256Hex(resumeToken)];
      this.sendError(ws, (JSON.parse(welcome) as { err: string }).err);
      ws.close(1008, "join refused");
      return;
    }
    attachment.clientId = clientId;
    attachment.name = joined.ok.name;
    attachment.joined = true;
    attachment.joinedAt = Date.now();
    ws.serializeAttachment(attachment);
    this.lastSeen.set(clientId, attachment.joinedAt);
    this.roster[clientId] = attachment.name;
    if (this.pending === clientId) {
      this.pending = null;
      this.writeKv("pending", "");
    }
    this.writeRoster();
    this.writeSecrets();
    ws.send(JSON.stringify({ ...(JSON.parse(welcome) as object), resume_token: resumeToken }));
    this.broadcast(JSON.stringify({ type: "peer_joined", name: attachment.name }), ws);
    this.markDirty();
    await this.schedule();
  }

  private async drop(ws: WebSocket): Promise<void> {
    const attachment = ws.deserializeAttachment() as Attachment | null;
    if (attachment === null || !attachment.joined) return;
    // Close and error can both arrive; only the first one counts.
    attachment.joined = false;
    try {
      ws.serializeAttachment(attachment);
    } catch {
      // socket already torn down
    }
    this.lastSeen.delete(attachment.clientId);
    this.releasePlayer(attachment.clientId, attachment.name, ws);
    this.writeRoster();
    this.writeSecrets();
    this.markDirty();
    if (this.joinedCount() === 0) this.flush();
    await this.schedule();
  }

  /**
   * Remove a player from the room and tell everyone left, in the axum
   * server's order: peer_left, snapshot, then the upload token.
   */
  private releasePlayer(clientId: string, name: string, except?: WebSocket): void {
    const wasListed = clientId in this.roster;
    const left = JSON.parse(this.wasm(() => this.room.leave(clientId, name))) as Reply<{
      name: string;
      snapshot: unknown;
      upload_assignment: { client_id: string; sha256: string } | null;
    }>;
    delete this.roster[clientId];
    // The player's resume secret is kept so a refresh or a reconnect after a
    // deploy gets the same identity back; pruneSecrets bounds how many stay.
    if ("err" in left) return;
    if (wasListed) {
      this.broadcast(JSON.stringify({ type: "peer_left", name: left.ok.name }), except);
    }
    if (left.ok.snapshot !== null) {
      this.broadcast(JSON.stringify({ type: "snapshot", snapshot: left.ok.snapshot }), except);
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
        except,
      );
    }
  }

  /**
   * Resume secrets of players who are gone are kept for reconnects, but only
   * the newest few; anyone present (or the unclaimed creator) always keeps theirs.
   */
  private pruneSecrets(): void {
    const keep = new Set([...Object.keys(this.roster), ...(this.pending ? [this.pending] : [])]);
    const entries = Object.entries(this.secrets);
    let surplus = entries.length - MAX_KEPT_SECRETS;
    for (const [hash, id] of entries) {
      if (surplus <= 0) break;
      if (!keep.has(id)) {
        delete this.secrets[hash];
        surplus--;
      }
    }
  }

  /** Send `eventJson` to every joined socket allowed to see it. */
  private broadcast(eventJson: string, except?: WebSocket): void {
    for (const socket of this.ctx.getWebSockets()) {
      if (socket === except) continue;
      const attachment = socket.deserializeAttachment() as Attachment | null;
      if (
        attachment?.joined &&
        this.wasm(() => Room.visible_to(eventJson, attachment.clientId))
      ) {
        try {
          socket.send(eventJson);
        } catch {
          // a dead socket is cleaned up by its close event or the sweep
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

  private joinedCount(): number {
    return this.attachments().filter((a) => a.joined).length;
  }

  /**
   * A trap in the wasm module leaves it unusable (a panic aborts, and the
   * object's borrow flag stays set), so reset the whole Durable Object: it
   * reloads the last flushed snapshot and clients reconnect.
   */
  private wasm<T>(call: () => T): T {
    try {
      return call();
    } catch (error) {
      console.error("room core failed; resetting the room", String(error));
      this.ctx.abort("room core failed");
      throw error;
    }
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
    this.writeKv("snapshot", this.wasm(() => this.room.snapshot_json()));
    this.writeRoster();
    this.dirtySince = null;
  }

  private writeRoster(): void {
    this.writeKv("roster", JSON.stringify(this.roster));
  }

  private writeSecrets(): void {
    this.writeKv("secrets", JSON.stringify(this.secrets));
  }

  private setUploadToken(token: string): void {
    this.uploadToken = token;
    this.writeKv("upload_token", token);
  }

  /** Arm one alarm for the earliest of flush, playback deadline and sweep. */
  private async schedule(): Promise<void> {
    const now = Date.now();
    const target = alarmTarget({
      now,
      dirtySince: this.dirtySince,
      flushMs: this.flushMs,
      deadline: this.wasm(() => this.room.next_tick_deadline(now)),
      joinedSockets: this.joinedCount(),
      sweepMs: Math.min(MAX_SWEEP_MS, Math.max(250, Math.floor(this.idleSocketMs / 3))),
      minGapMs: MIN_ALARM_GAP_MS,
      maxAheadMs: MAX_ALARM_AHEAD_MS,
    });
    if (target === null) return;
    const current = await this.ctx.storage.getAlarm();
    if (shouldSetAlarm(current, target, now)) await this.ctx.storage.setAlarm(target);
  }

  private readKv(key: string): string | null {
    const rows = this.ctx.storage.sql.exec("SELECT v FROM kv WHERE k = ?", key).toArray();
    const value = rows.length === 0 ? null : (rows[0]!.v as string);
    return value === "" ? null : value;
  }

  private writeKv(key: string, value: string): void {
    this.ctx.storage.sql.exec(
      "INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
      key,
      value,
    );
  }
}
