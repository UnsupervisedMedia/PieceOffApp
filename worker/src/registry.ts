import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { newCode, numVar, randomB64url, sha256Hex } from "./http.ts";
import { shouldSetAlarm } from "./schedule.ts";

const GIB = 1024 * 1024 * 1024;
const DAY_MS = 86_400_000;
const DEFAULT_LINK_TTL_MS = 600_000;
const DEFAULT_ROOMS_PER_DAY = 30;
const DEFAULT_DEVICE_BYTES_PER_DAY = 2 * GIB;
/** Under R2's 10 GB free tier, leaving room for parts R2 holds briefly. */
export const DEFAULT_GLOBAL_MEDIA_BYTES = 8 * GIB;
/** Pending links held at once; far above honest use, bounds a flood. */
const MAX_PENDING_LINKS = 5_000;
/** Characters of a device key's hash a room records as `creator_device`. */
export const DEVICE_PREFIX_LENGTH = 16;
export const DEVICE_KEY_RE = /^pok_[A-Za-z0-9_-]{43}$/;
export const MEDIA_FULL_ERROR =
  "PieceOff Online is out of media space right now. Try a smaller file or try later.";
const DEVICE_MEDIA_ERROR =
  "This computer has uploaded as much media as it may today. Try again tomorrow.";
const RELINK_ERROR = "This computer is no longer linked to PieceOff Online. Link it again.";

export type Charged = { ok: true; device: string } | { ok: false; status: number; error: string; relink?: boolean };
export type Reserved = { ok: true } | { ok: false; status: number; error: string };
export type Polled =
  | { status: "missing" }
  | { status: "pending" }
  | { status: "approved"; deviceKey: string };

const today = (now: number): number => Math.floor(now / DAY_MS);

/**
 * The service-wide registry: one instance (`idFromName("registry")`).
 *
 * - Device links: a desktop app asks for a link code, the player approves it
 *   in a browser (Turnstile), the app collects a device key once. Only
 *   hashes of the poll secret and of device keys are stored. The key is
 *   minted when the app collects it, so its plain value is never stored.
 * - Device keys: daily room and media-byte counters, a revoked flag.
 * - The global media budget, kept per room (`room_media`) rather than as one
 *   sum so a room can report its true total and correct any drift a crash
 *   left behind (`reconcileRoom`). Rows of rooms that vanished without
 *   saying so are dropped after the longest a live room can stay silent.
 */
export class RegistryDO extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.ctx.blockConcurrencyWhile(async () => {
      const sql = this.ctx.storage.sql;
      sql.exec(`CREATE TABLE IF NOT EXISTS links (
        code TEXT PRIMARY KEY, poll_hash TEXT NOT NULL, device_name TEXT NOT NULL,
        created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, approved INTEGER NOT NULL DEFAULT 0)`);
      sql.exec(`CREATE TABLE IF NOT EXISTS devices (
        hash TEXT PRIMARY KEY, prefix TEXT NOT NULL, name TEXT NOT NULL,
        created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL, revoked INTEGER NOT NULL DEFAULT 0,
        day INTEGER NOT NULL DEFAULT 0, rooms_today INTEGER NOT NULL DEFAULT 0,
        media_bytes_today INTEGER NOT NULL DEFAULT 0, rooms_total INTEGER NOT NULL DEFAULT 0,
        media_bytes_total INTEGER NOT NULL DEFAULT 0)`);
      sql.exec("CREATE INDEX IF NOT EXISTS devices_prefix ON devices (prefix)");
      sql.exec(`CREATE TABLE IF NOT EXISTS room_media (
        room TEXT PRIMARY KEY, device TEXT NOT NULL, bytes INTEGER NOT NULL, updated_at INTEGER NOT NULL)`);
    });
  }

  private get sql(): SqlStorage {
    return this.ctx.storage.sql;
  }

  private linkTtlMs(): number {
    return numVar(this.env.LINK_TTL_MS, DEFAULT_LINK_TTL_MS);
  }

  // ---- Device links --------------------------------------------------------

  async createLink(deviceName: string): Promise<
    { ok: true; code: string; pollSecret: string; expiresIn: number } | { ok: false; error: string }
  > {
    const now = Date.now();
    const pending = Number(this.sql.exec("SELECT COUNT(*) AS n FROM links WHERE expires_at > ?", now).one().n);
    if (pending >= MAX_PENDING_LINKS) return { ok: false, error: "Too many computers are linking right now. Try again soon." };
    const pollSecret = randomB64url(32);
    const pollHash = await sha256Hex(pollSecret);
    const ttl = this.linkTtlMs();
    for (let attempt = 0; attempt < 5; attempt++) {
      const code = newCode();
      if (this.sql.exec("SELECT 1 FROM links WHERE code = ?", code).toArray().length > 0) continue;
      this.sql.exec(
        "INSERT INTO links (code, poll_hash, device_name, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        code, pollHash, deviceName, now, now + ttl,
      );
      await this.arm(now + ttl);
      return { ok: true, code, pollSecret, expiresIn: Math.round(ttl / 1000) };
    }
    return { ok: false, error: "Could not pick a free link code. Please try again." };
  }

  /** The browser step. Approval also gives the app a fresh window to collect the key. */
  async approveLink(code: string): Promise<"ok" | "missing" | "already"> {
    const now = Date.now();
    const rows = this.sql.exec("SELECT approved, expires_at FROM links WHERE code = ?", code).toArray();
    if (rows.length === 0 || (rows[0]!.expires_at as number) <= now) return "missing";
    if (rows[0]!.approved) return "already";
    const expires = Math.max(rows[0]!.expires_at as number, now + this.linkTtlMs());
    this.sql.exec("UPDATE links SET approved = 1, expires_at = ? WHERE code = ?", expires, code);
    return "ok";
  }

  /** The app's poll. An approved link hands out its key once and is deleted. */
  async pollLink(code: string, pollSecret: string): Promise<Polled> {
    const now = Date.now();
    const rows = this.sql.exec("SELECT * FROM links WHERE code = ?", code).toArray();
    if (rows.length === 0 || (rows[0]!.expires_at as number) <= now) return { status: "missing" };
    const link = rows[0]!;
    const presented = new TextEncoder().encode(await sha256Hex(pollSecret));
    const stored = new TextEncoder().encode(link.poll_hash as string);
    if (presented.length !== stored.length || !crypto.subtle.timingSafeEqual(presented, stored)) {
      return { status: "missing" };
    }
    if (!link.approved) return { status: "pending" };
    const deviceKey = `pok_${randomB64url(32)}`;
    const hash = await sha256Hex(deviceKey);
    this.sql.exec(
      "INSERT INTO devices (hash, prefix, name, created_at, last_used_at, day) VALUES (?, ?, ?, ?, ?, ?)",
      hash, hash.slice(0, DEVICE_PREFIX_LENGTH), link.device_name as string, now, now, today(now),
    );
    this.sql.exec("DELETE FROM links WHERE code = ?", code);
    return { status: "approved", deviceKey };
  }

  // ---- Device keys ---------------------------------------------------------

  private device(where: "hash" | "prefix", value: string): Record<string, SqlStorageValue> | null {
    const rows = this.sql.exec(`SELECT * FROM devices WHERE ${where} = ?`, value).toArray();
    return rows[0] ?? null;
  }

  /** Counters as of today: a new UTC day starts at zero. */
  private rollDay(row: Record<string, SqlStorageValue>, now: number): { rooms: number; bytes: number } {
    return row.day === today(now)
      ? { rooms: row.rooms_today as number, bytes: row.media_bytes_today as number }
      : { rooms: 0, bytes: 0 };
  }

  /** Check a device key and count one room against its daily allowance. */
  async chargeRoom(keyHash: string): Promise<Charged> {
    const now = Date.now();
    const row = this.device("hash", keyHash);
    if (row === null || row.revoked) return { ok: false, status: 401, error: RELINK_ERROR, relink: true };
    const used = this.rollDay(row, now);
    const cap = numVar(this.env.DEVICE_ROOMS_PER_DAY, DEFAULT_ROOMS_PER_DAY);
    if (used.rooms >= cap) {
      return { ok: false, status: 429, error: "This computer has created as many rooms as it may today. Try again tomorrow." };
    }
    this.sql.exec(
      "UPDATE devices SET day = ?, rooms_today = ?, media_bytes_today = ?, rooms_total = rooms_total + 1, last_used_at = ? WHERE hash = ?",
      today(now), used.rooms + 1, used.bytes, now, keyHash,
    );
    return { ok: true, device: keyHash.slice(0, DEVICE_PREFIX_LENGTH) };
  }

  /** Give back a room charge whose room could not be made. */
  async refundRoom(device: string): Promise<void> {
    const row = this.device("prefix", device);
    if (row === null || row.day !== today(Date.now())) return;
    this.sql.exec(
      "UPDATE devices SET rooms_today = MAX(rooms_today - 1, 0), rooms_total = MAX(rooms_total - 1, 0) WHERE prefix = ?",
      device,
    );
  }

  /** Revoke (or restore) by key hash prefix or by the key itself. Returns how many matched. */
  async setRevoked(match: { prefix?: string; keyHash?: string }, revoked: boolean): Promise<number> {
    const flag = revoked ? 1 : 0;
    if (match.keyHash) {
      return this.sql.exec("UPDATE devices SET revoked = ? WHERE hash = ?", flag, match.keyHash).rowsWritten;
    }
    if (match.prefix) {
      return this.sql.exec("UPDATE devices SET revoked = ? WHERE prefix = ?", flag, match.prefix).rowsWritten;
    }
    return 0;
  }

  // ---- Global media budget -------------------------------------------------

  private globalBytes(): number {
    return Number(this.sql.exec("SELECT COALESCE(SUM(bytes), 0) AS n FROM room_media").one().n);
  }

  /** Reserve `bytes` for a room's upload, charging its creator device's day. */
  async reserveMedia(room: string, device: string, bytes: number): Promise<Reserved> {
    const now = Date.now();
    if (!Number.isSafeInteger(bytes) || bytes <= 0) return { ok: false, status: 400, error: "Bad media size" };
    const quota = numVar(this.env.MEDIA_GLOBAL_QUOTA_BYTES, DEFAULT_GLOBAL_MEDIA_BYTES);
    if (this.globalBytes() + bytes > quota) return { ok: false, status: 507, error: MEDIA_FULL_ERROR };
    const row = device ? this.device("prefix", device) : null;
    if (row !== null) {
      const used = this.rollDay(row, now);
      const cap = numVar(this.env.DEVICE_MEDIA_BYTES_PER_DAY, DEFAULT_DEVICE_BYTES_PER_DAY);
      if (used.bytes + bytes > cap) return { ok: false, status: 507, error: DEVICE_MEDIA_ERROR };
      this.sql.exec(
        "UPDATE devices SET day = ?, rooms_today = ?, media_bytes_today = ?, media_bytes_total = media_bytes_total + ?, last_used_at = ? WHERE prefix = ?",
        today(now), used.rooms, used.bytes + bytes, bytes, now, device,
      );
    }
    this.sql.exec(
      `INSERT INTO room_media (room, device, bytes, updated_at) VALUES (?, ?, ?, ?)
       ON CONFLICT (room) DO UPDATE SET bytes = bytes + excluded.bytes, updated_at = excluded.updated_at`,
      room, device, bytes, now,
    );
    await this.arm(now + this.orphanMs());
    return { ok: true };
  }

  /** Free a reservation (abort, failed start). `refundDevice` gives back the device's day too. */
  async releaseMedia(room: string, device: string, bytes: number, refundDevice: boolean): Promise<void> {
    const now = Date.now();
    if (!Number.isSafeInteger(bytes) || bytes <= 0) return;
    this.sql.exec(
      "UPDATE room_media SET bytes = MAX(bytes - ?, 0), updated_at = ? WHERE room = ?",
      bytes, now, room,
    );
    this.sql.exec("DELETE FROM room_media WHERE room = ? AND bytes = 0", room);
    const row = refundDevice && device ? this.device("prefix", device) : null;
    if (row !== null && row.day === today(now)) {
      this.sql.exec(
        "UPDATE devices SET media_bytes_today = MAX(media_bytes_today - ?, 0) WHERE prefix = ?",
        bytes, device,
      );
    }
  }

  /** A room's true total (stored plus in flight) replaces whatever drifted. */
  async reconcileRoom(room: string, device: string, bytes: number): Promise<void> {
    const now = Date.now();
    if (!Number.isSafeInteger(bytes) || bytes <= 0) {
      this.sql.exec("DELETE FROM room_media WHERE room = ?", room);
      return;
    }
    this.sql.exec(
      `INSERT INTO room_media (room, device, bytes, updated_at) VALUES (?, ?, ?, ?)
       ON CONFLICT (room) DO UPDATE SET bytes = excluded.bytes, updated_at = excluded.updated_at`,
      room, device, bytes, now,
    );
    await this.arm(now + this.orphanMs());
  }

  /** The room was deleted with all its media. */
  async forgetRoom(room: string): Promise<void> {
    this.sql.exec("DELETE FROM room_media WHERE room = ?", room);
  }

  /** Bytes counted against the global budget (for tests and operators). */
  async mediaUsage(): Promise<number> {
    return this.globalBytes();
  }

  // ---- Housekeeping --------------------------------------------------------

  /**
   * A live room reports at least every reconcile interval while it is in
   * use and is deleted at most `IDLE_CLEANUP_MS` after it empties, so a row
   * silent for longer than both plus a day belongs to a room that is gone.
   */
  private orphanMs(): number {
    return numVar(this.env.IDLE_CLEANUP_MS, 3_600_000) + DAY_MS;
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    this.sql.exec("DELETE FROM links WHERE expires_at <= ?", now);
    this.sql.exec("DELETE FROM room_media WHERE updated_at < ?", now - this.orphanMs());
    const next = this.sql.exec("SELECT MIN(expires_at) AS t FROM links").one().t as number | null;
    const oldest = this.sql.exec("SELECT MIN(updated_at) AS t FROM room_media").one().t as number | null;
    const targets = [next, oldest === null ? null : oldest + this.orphanMs() + 1].filter(
      (t): t is number => t !== null,
    );
    if (targets.length > 0) await this.ctx.storage.setAlarm(Math.max(Math.min(...targets), now + 1_000));
  }

  /** Make sure an alarm fires by `target`; moving one costs a row, so only when needed. */
  private async arm(target: number): Promise<void> {
    const now = Date.now();
    if (shouldSetAlarm(await this.ctx.storage.getAlarm(), target, now)) await this.ctx.storage.setAlarm(target);
  }
}
