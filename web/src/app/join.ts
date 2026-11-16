// Joining a hosted room, the way the desktop client does it
// (crates/pieceoff-app/src/online_ui.rs, net.rs):
//
// - A full room (core: "This room already has 8 players") turns into watching:
//   one new connection as a spectator, made by this flow (the refused player
//   socket is closed first, so it never retries).
// - A taken name (or one the public filter refuses) asks for another name and
//   waits. Nothing joins again until the player submits one.
// - A saved resume token for the room (this tab, see names.ts) goes straight
//   back in with the same identity: a reload resumes, and the creator stays host.
// - Everything that can never pass (banned, room gone, paused, version
//   mismatch, opened elsewhere, kicked) ends in a state with no socket left.
import { RoomState } from '../net/roomState';
import { RoomSocket, type FatalReason, type RoomSocketOptions } from '../net/socket';
import type { ServerMessage } from '../protocol';
import { wsUrl } from './config';
import {
  browserStorage,
  cleanName,
  clearResume,
  loadName,
  loadResume,
  playerKey,
  saveName,
  saveResume,
  type AppStorage,
} from './names';
import { Store, type Observable } from './observable';
import { public_name_allowed } from '../../wasm/pieceoff_web_wasm.js';

/** Core's refusal when a room has MAX_PLAYERS players (membership.rs). */
export const ROOM_FULL = 'This room already has 8 players';
/** Core's refusal for a blocked name in a listed room (moderation.rs PUBLIC_NAME_REFUSED). */
export const PUBLIC_NAME_REFUSED = 'Pick a different name for public rooms';

// ---- room codes -----------------------------------------------------------

/** Room and link codes: 8 characters, no I, O, 0 or 1 (worker/src/http.ts CODE_RE). */
export const CODE_RE = /^[ABCDEFGHJKLMNPQRSTUVWXYZ23456789]{8}$/;

/** The normalised code (trimmed, spaces and dashes dropped, upper case), or null. */
export function parseCode(raw: string | null | undefined): string | null {
  if (typeof raw !== 'string' || raw.length > 64) return null;
  const c = raw.trim().replace(/[\s-]/g, '').toUpperCase();
  return CODE_RE.test(c) ? c : null;
}

/** The code of a `/join?c=CODE` link, from `location.search`. */
export function codeFromSearch(search: string): string | null {
  try {
    return parseCode(new URLSearchParams(search).get('c'));
  } catch {
    return null;
  }
}

// ---- states -----------------------------------------------------------------

type Welcome = Extract<ServerMessage, { type: 'welcome' }>;

export interface Joined {
  code: string;
  name: string;
  socket: RoomSocket;
  /** Fed from the socket since before it connected (welcome, chat history, ...). */
  room: RoomState;
  welcome: Welcome;
}

export type JoinState =
  | { kind: 'idle' }
  | { kind: 'asking-name'; code: string; listed: boolean; spectator: boolean; suggested: string }
  | {
      kind: 'checking';
      code: string;
      name: string;
      spectator: boolean;
      /** The last attempt was rate limited; the socket waits on its backoff. */
      rateLimited: boolean;
      /** A server refusal that may pass on a later try (shown, not fatal). */
      message: string | null;
    }
  | ({ kind: 'joined-player' } & Joined)
  | ({ kind: 'joined-spectator' } & Joined)
  /** Wanted to play, the room had 8 players, so watching instead. */
  | ({ kind: 'room-full-spectating' } & Joined)
  | {
      kind: 'name-taken';
      code: string;
      name: string;
      listed: boolean;
      spectator: boolean;
      /** 'taken': another player holds it; 'not-allowed': the public-name filter. */
      reason: 'taken' | 'not-allowed';
      /** Verbatim server (or core) text. */
      message: string;
    }
  | { kind: 'room-not-found'; code: string; invalid: boolean }
  | { kind: 'fatal'; code: string; reason: FatalReason };

export type JoinKind = JoinState['kind'];

export interface StartOptions {
  /** The room is known to be public (picked from the lobby): run the name filter first. */
  listed?: boolean;
  /** Watch instead of play. */
  spectator?: boolean;
}

export interface JoinDeps {
  api: string;
  storage?: AppStorage;
  publicNameAllowed?: (name: string) => boolean;
  /** Passed to every RoomSocket (tests: makeSocket, timers, probe, visibility). */
  socket?: Partial<Omit<RoomSocketOptions, 'url' | 'join' | 'resumeToken'>>;
}

export interface JoinFlow {
  start(code: string, o?: StartOptions): void;
  submitName(n: string): void;
  /** Close the connection and forget this tab's place in the room. */
  leave(): void;
  readonly state: Observable<JoinState>;
}

export function joinFlow(deps: JoinDeps): JoinFlow {
  const storage = deps.storage ?? browserStorage();
  const allowed = deps.publicNameAllowed ?? ((n: string) => public_name_allowed(n));
  const store = new Store<JoinState>({ kind: 'idle' });
  let socket: RoomSocket | null = null;
  let generation = 0;
  let listed = false;

  const drop = () => {
    generation++;
    const s = socket;
    socket = null;
    s?.close();
  };

  const connect = (code: string, name: string, spectator: boolean, token: string | null, fullSwitch: boolean) => {
    drop();
    const gen = generation;
    const live = () => gen === generation;
    const room = new RoomState();
    let lastError: string | null = null;
    let joined = false;
    store.set({ kind: 'checking', code, name, spectator, rateLimited: false, message: null });

    const s = new RoomSocket({
      ...deps.socket,
      url: wsUrl(deps.api, code),
      join: {
        invite_code: code,
        name,
        // Hosted rooms are built-in only and built-ins are always available,
        // so the join carries no media of ours.
        media_sha256: '',
        media_extension: '',
        spectator,
        player_key: playerKey(storage),
      },
      resumeToken: token,
    });
    socket = s;
    room.attach(s);

    s.onMessage((m) => {
      if (!live()) return;
      if (m.type === 'welcome') {
        joined = true;
        const kind = m.spectator ? (fullSwitch ? 'room-full-spectating' : 'joined-spectator') : 'joined-player';
        if (s.resumeToken) saveResume(storage, code, { token: s.resumeToken, name: m.name, spectator: m.spectator });
        if (!m.spectator) saveName(storage, m.name);
        // A reconnect's welcome only refreshes the token; the state stands.
        if (store.get().kind === 'checking') store.set({ kind, code, name: m.name, socket: s, room, welcome: m });
        return;
      }
      if (m.type !== 'error' || joined) return;
      lastError = m.message;
      if (m.message === ROOM_FULL && !spectator) {
        // Watch instead, like desktop's "This room is full; you can still watch".
        connect(code, name, true, token, true);
        return;
      }
      const cur = store.get();
      if (cur.kind === 'checking' && cur.message !== m.message) store.set({ ...cur, message: m.message });
    });

    s.onState((state, why) => {
      if (!live()) return;
      if (state === 'reconnecting' && !joined) {
        const cur = store.get();
        if (cur.kind === 'checking') store.set({ ...cur, rateLimited: why === 'rate-limited' });
        return;
      }
      if (state !== 'fatal' || !why) return;
      socket = null;
      generation++;
      if (why === 'name-taken' && !joined) {
        const notAllowed = lastError?.startsWith(PUBLIC_NAME_REFUSED) ?? false;
        clearResume(storage, code);
        store.set({
          kind: 'name-taken',
          code,
          name,
          listed: listed || notAllowed,
          spectator,
          reason: notAllowed ? 'not-allowed' : 'taken',
          message: lastError ?? 'That name is taken in this room. Pick another name.',
        });
        return;
      }
      if (why === 'room-gone' && !joined) {
        clearResume(storage, code);
        store.set({ kind: 'room-not-found', code, invalid: false });
        return;
      }
      if (why === 'kicked' || why === 'banned' || why === 'room-gone') clearResume(storage, code);
      store.set({ kind: 'fatal', code, reason: why });
    });
  };

  return {
    state: store,

    start(raw, o = {}) {
      drop();
      const code = parseCode(raw);
      if (!code) {
        store.set({ kind: 'room-not-found', code: '', invalid: true });
        return;
      }
      listed = o.listed === true;
      const resume = loadResume(storage, code);
      if (resume) {
        connect(code, resume.name, resume.spectator, resume.token, false);
        return;
      }
      store.set({
        kind: 'asking-name',
        code,
        listed,
        spectator: o.spectator === true,
        suggested: loadName(storage),
      });
    },

    submitName(n) {
      const cur = store.get();
      if (cur.kind !== 'asking-name' && cur.kind !== 'name-taken') return;
      const name = cleanName(n);
      if (cur.listed && !allowed(name)) {
        store.set({
          kind: 'name-taken',
          code: cur.code,
          name,
          listed: true,
          spectator: cur.spectator,
          reason: 'not-allowed',
          message: PUBLIC_NAME_REFUSED,
        });
        return;
      }
      connect(cur.code, name, cur.spectator, null, false);
    },

    leave() {
      const cur = store.get();
      drop();
      if ('code' in cur && cur.code) clearResume(storage, cur.code);
      store.set({ kind: 'idle' });
    },
  };
}
