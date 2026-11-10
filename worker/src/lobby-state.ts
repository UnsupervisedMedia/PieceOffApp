/**
 * Pure rules for the public lobby: what an entry looks like, how display text
 * is tidied, how the list is ordered, and when entries expire. No bindings,
 * so it is tested directly.
 */

export interface LobbyEntry {
  code: string;
  /** The built-in loop's bundled title; never text a client supplied. */
  title: string;
  host_name: string;
  players: number;
  max_players: number;
  spectators: number;
  pieces: number;
  placed: number;
  cut_style: string;
  created_at: number;
  updated_at: number;
}

export const MAX_HOST_NAME = 24;
export const MAX_LOBBY_ROWS = 100;

/**
 * Display text safe to show strangers: control, format (including
 * text-direction overrides), private-use and line/paragraph separator
 * characters go, whitespace runs become one space, trimmed, at most `max`
 * characters (code points).
 */
export function sanitizeText(text: unknown, max: number): string {
  if (typeof text !== "string") return "";
  const cleaned = text
    .replace(/[\p{Cc}\p{Cf}\p{Co}\p{Cs}\p{Zl}\p{Zp}]/gu, " ")
    .replace(/\s+/gu, " ")
    .trim();
  return Array.from(cleaned).slice(0, max).join("").trim();
}

const CODE_RE = /^[A-HJ-NP-Z2-9]{8}$/;
const CUT_STYLES = new Set(["jigsaw", "hexagon", "triangle"]);

const count = (value: unknown, cap: number): number =>
  Number.isSafeInteger(value) && (value as number) >= 0 ? Math.min(value as number, cap) : 0;

/** Clamp and tidy an entry from a room; null if it cannot be shown. */
export function normalizeEntry(raw: Partial<LobbyEntry>, now: number): LobbyEntry | null {
  if (typeof raw.code !== "string" || !CODE_RE.test(raw.code)) return null;
  const title = sanitizeText(raw.title, 80);
  if (title === "") return null;
  const pieces = count(raw.pieces, 100_000);
  const created = Number.isSafeInteger(raw.created_at) ? (raw.created_at as number) : now;
  return {
    code: raw.code,
    title,
    host_name: sanitizeText(raw.host_name, MAX_HOST_NAME),
    players: count(raw.players, 64),
    max_players: count(raw.max_players, 64),
    spectators: count(raw.spectators, 64),
    pieces,
    placed: Math.min(count(raw.placed, 100_000), pieces),
    cut_style: typeof raw.cut_style === "string" && CUT_STYLES.has(raw.cut_style) ? raw.cut_style : "jigsaw",
    created_at: created,
    updated_at: Number.isSafeInteger(raw.updated_at) ? (raw.updated_at as number) : now,
  };
}

/** Whether two entries differ in anything but `updated_at`. */
export function entryChanged(a: LobbyEntry | undefined, b: LobbyEntry): boolean {
  if (a === undefined) return true;
  return JSON.stringify({ ...a, updated_at: 0 }) !== JSON.stringify({ ...b, updated_at: 0 });
}

/** Rooms with a free player slot first, then fuller rooms, then newest; at most 100. */
export function sortLobby(entries: LobbyEntry[]): LobbyEntry[] {
  const free = (e: LobbyEntry): number => (e.players < e.max_players ? 1 : 0);
  return [...entries]
    .sort((a, b) => free(b) - free(a) || b.players - a.players || b.created_at - a.created_at || (a.code < b.code ? -1 : 1))
    .slice(0, MAX_LOBBY_ROWS);
}

/** Whether a room last heard from at `heartbeatAt` has dropped out of the lobby. */
export function expired(heartbeatAt: number, now: number, expiryMs: number): boolean {
  return now - heartbeatAt >= expiryMs;
}

/**
 * What a room should do about the lobby now: send an upsert, send a remove,
 * or nothing. Upserts are throttled; a remove is never delayed.
 */
export function lobbyAction(input: {
  /** The entry the room would show, or null if it should not be listed. */
  wanted: boolean;
  /** The lobby currently has (or may have) this room. */
  listed: boolean;
  /** Something lobby-visible changed since the last upsert. */
  dirty: boolean;
  now: number;
  lastSentAt: number;
  throttleMs: number;
  heartbeatMs: number;
}): "upsert" | "remove" | "none" {
  if (!input.wanted) return input.listed ? "remove" : "none";
  const since = input.now - input.lastSentAt;
  if (!input.listed || input.dirty) return since >= input.throttleMs ? "upsert" : "none";
  return since >= input.heartbeatMs ? "upsert" : "none";
}
