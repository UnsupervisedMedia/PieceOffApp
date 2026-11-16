// Display names, the device's player key and per-room resume tokens.
//
// - Names are tidied exactly like core's `clean_name` (sanitize.rs): control
//   characters dropped, the first 24 characters kept, then trimmed; empty
//   becomes "Guest". The public-name blocklist is core's, through WASM; the
//   server stays the authority.
// - `player_key`: 128 random bits as hex, made once per browser and kept in
//   localStorage, so a name claim survives a refresh.
// - Resume tokens are secrets that prove "I am this player" (for the creator:
//   "I am the host"). They live in sessionStorage, keyed by room code: a reload
//   of this tab resumes, a new tab or a closed browser does not.
import { public_name_allowed } from '../../wasm/pieceoff_web_wasm.js';

export const MAX_NAME_CHARS = 24;
const GUEST = 'Guest';

const KEY_PLAYER = 'pieceoff.web.playerKey';
const KEY_NAME = 'pieceoff.web.name';
const KEY_RESUME = 'pieceoff.web.resume.';
const PLAYER_KEY_RE = /^[0-9a-f]{32}$/;

export interface AppStorage {
  local: Storage | null;
  session: Storage | null;
}

export interface ResumeEntry {
  token: string;
  name: string;
  spectator: boolean;
}

/** The browser's storage, or null where it is unavailable (privacy modes, sandboxed frames). */
export function browserStorage(): AppStorage {
  const get = (k: 'localStorage' | 'sessionStorage'): Storage | null => {
    try {
      return (globalThis as unknown as Record<string, Storage | undefined>)[k] ?? null;
    } catch {
      return null;
    }
  };
  return { local: get('localStorage'), session: get('sessionStorage') };
}

/** A Map-backed `Storage` (tests, and a stand-in when the real one is missing). */
export function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k: string) => m.get(k) ?? null,
    key: (i: number) => [...m.keys()][i] ?? null,
    removeItem: (k: string) => void m.delete(k),
    setItem: (k: string, v: string) => void m.set(k, String(v)),
  };
}

function read(s: Storage | null, k: string): string | null {
  try {
    return s?.getItem(k) ?? null;
  } catch {
    return null;
  }
}

function write(s: Storage | null, k: string, v: string | null): void {
  try {
    if (v === null) s?.removeItem(k);
    else s?.setItem(k, v);
  } catch {
    // full or blocked: the value just is not remembered
  }
}

const CONTROL = /\p{Cc}/gu;

/** Core's `clean_name`, character for character. */
export function cleanName(name: string): string {
  const kept = [...String(name).replace(CONTROL, '')].slice(0, MAX_NAME_CHARS).join('');
  const t = kept.trim();
  return t === '' ? GUEST : t;
}

/** Whether `name` may be used: listed rooms run core's public-name filter; invite rooms do not ask. */
export function nameAllowed(name: string, listed: boolean): boolean {
  return !listed || public_name_allowed(name);
}

// Keys that storage refused to keep, so the page still uses one key for its life.
const unsaved = new WeakMap<object, string>();
const NO_STORAGE = {};

function randomHex128(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

/** This browser's player key: created once, then the same on every visit. */
export function playerKey(s: AppStorage): string {
  const stored = read(s.local, KEY_PLAYER);
  if (stored && PLAYER_KEY_RE.test(stored)) return stored;
  const slot = s.local ?? NO_STORAGE;
  const kept = unsaved.get(slot);
  if (kept) return kept;
  const key = randomHex128();
  write(s.local, KEY_PLAYER, key);
  if (read(s.local, KEY_PLAYER) !== key) unsaved.set(slot, key);
  return key;
}

export function loadName(s: AppStorage): string {
  const n = read(s.local, KEY_NAME);
  return n ? [...n].slice(0, MAX_NAME_CHARS).join('') : '';
}

export function saveName(s: AppStorage, name: string): void {
  write(s.local, KEY_NAME, cleanName(name));
}

export function loadResume(s: AppStorage, code: string): ResumeEntry | null {
  const raw = read(s.session, KEY_RESUME + code);
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as Partial<ResumeEntry> | null;
    if (
      v &&
      typeof v.token === 'string' &&
      v.token.length > 0 &&
      v.token.length <= 256 &&
      typeof v.name === 'string' &&
      typeof v.spectator === 'boolean'
    ) {
      return { token: v.token, name: cleanName(v.name), spectator: v.spectator };
    }
  } catch {
    // unreadable: forget it below
  }
  write(s.session, KEY_RESUME + code, null);
  return null;
}

export function saveResume(s: AppStorage, code: string, e: ResumeEntry): void {
  write(s.session, KEY_RESUME + code, JSON.stringify({ token: e.token, name: e.name, spectator: e.spectator }));
}

export function clearResume(s: AppStorage, code: string): void {
  write(s.session, KEY_RESUME + code, null);
}
