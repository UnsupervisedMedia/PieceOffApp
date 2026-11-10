import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { numVar } from "./http.ts";
import { type LobbyEntry, entryChanged, expired, normalizeEntry, sortLobby } from "./lobby-state.ts";

const DEFAULT_EXPIRY_MS = 150_000;
/** Bounds memory and storage if something floods the lobby. */
const MAX_ENTRIES = 2_000;

/**
 * The public lobby: one instance (`idFromName("lobby")`). Listed rooms push
 * entries here; `GET /api/lobby` reads them.
 *
 * Cost: heartbeats only touch memory. A row is written when an entry's
 * content changes, and deleted when it is removed or expires, so a quiet
 * listed room costs no writes. After an eviction the stored entries come back
 * with a short grace period and live rooms refresh them with their next
 * heartbeat; dead ones expire. One alarm, at most every expiry period while
 * the lobby has entries, deletes rows of rooms that went away without saying.
 */
export class LobbyDO extends DurableObject<Env> {
  private entries = new Map<string, { entry: LobbyEntry; heartbeatAt: number }>();
  private readonly expiryMs: number;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.expiryMs = numVar(env.LOBBY_EXPIRY_MS, DEFAULT_EXPIRY_MS);
    this.ctx.blockConcurrencyWhile(async () => {
      this.ctx.storage.sql.exec(
        "CREATE TABLE IF NOT EXISTS entries (code TEXT PRIMARY KEY, entry TEXT NOT NULL, written_at INTEGER NOT NULL)",
      );
      const now = Date.now();
      // A live room heartbeats well within this grace; a dead one expires.
      const grace = Math.min(this.expiryMs, 90_000);
      for (const row of this.ctx.storage.sql.exec("SELECT entry, written_at FROM entries").toArray()) {
        try {
          const entry = JSON.parse(row.entry as string) as LobbyEntry;
          const heartbeatAt = Math.max(row.written_at as number, now - this.expiryMs + grace);
          this.entries.set(entry.code, { entry, heartbeatAt });
        } catch {
          // unreadable rows are dropped by the next prune
        }
      }
    });
  }

  /** Add or refresh a room. Returns false if the entry was refused. */
  async upsert(raw: Partial<LobbyEntry>): Promise<boolean> {
    const now = Date.now();
    const previous = this.entries.get(String(raw.code));
    const entry = normalizeEntry({ ...raw, updated_at: now }, now);
    if (entry === null) return false;
    if (previous === undefined && this.entries.size >= MAX_ENTRIES) {
      this.prune(now);
      if (this.entries.size >= MAX_ENTRIES) return false;
    }
    if (entryChanged(previous?.entry, entry)) {
      this.ctx.storage.sql.exec(
        "INSERT INTO entries (code, entry, written_at) VALUES (?, ?, ?) ON CONFLICT (code) DO UPDATE SET entry = excluded.entry, written_at = excluded.written_at",
        entry.code,
        JSON.stringify(entry),
        now,
      );
      this.entries.set(entry.code, { entry, heartbeatAt: now });
    } else {
      this.entries.set(entry.code, { entry: previous!.entry, heartbeatAt: now });
    }
    await this.arm(now);
    return true;
  }

  async remove(code: string): Promise<void> {
    if (!this.entries.delete(code)) return;
    this.ctx.storage.sql.exec("DELETE FROM entries WHERE code = ?", code);
  }

  async list(): Promise<LobbyEntry[]> {
    this.prune(Date.now());
    return sortLobby([...this.entries.values()].map((e) => e.entry));
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    this.prune(now);
    await this.arm(now);
  }

  private prune(now: number): void {
    for (const [code, { heartbeatAt }] of this.entries) {
      if (expired(heartbeatAt, now, this.expiryMs)) {
        this.entries.delete(code);
        this.ctx.storage.sql.exec("DELETE FROM entries WHERE code = ?", code);
      }
    }
  }

  /** Keep one coarse alarm while there is anything to expire. */
  private async arm(now: number): Promise<void> {
    if (this.entries.size === 0) return;
    const current = await this.ctx.storage.getAlarm();
    if (current === null || current <= now) await this.ctx.storage.setAlarm(now + this.expiryMs);
  }
}
