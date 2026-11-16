// One WebSocket to one hosted room: connect, join (always with the player
// key, and the resume token once we have one), resume after drops with a
// jittered, capped exponential backoff, stop for good on the refusals that
// can never pass, and keep drag/cursor traffic inside the server's limits.
//
// Everything that touches the outside world is injectable (socket factory,
// clock, timers, random, page visibility, HTTP probe) so tests run on fake
// timers with a hand-written WebSocket fake.
import {
  PROTOCOL_VERSION,
  checkServerProtocol,
  type ClientMessage,
  type ServerMessage,
} from '../protocol';

/** Why the socket stopped and will not reconnect by itself. */
export type FatalReason =
  | 'update-client' // the server speaks a newer protocol: reload / update
  | 'update-server' // the server speaks an older protocol than this client
  | 'kicked'
  | 'name-taken'
  | 'opened-elsewhere'
  | 'room-gone'
  | 'banned'
  | 'rate-limited' // only ever a hint on 'reconnecting'; see RoomSocket.onState
  | 'paused';

export type SocketState = 'connecting' | 'open' | 'reconnecting' | 'fatal';

/** The join as callers give it. `client_protocol` is not theirs to set: the
 *  socket always sends this build's PROTOCOL_VERSION (and the resume token). */
export type JoinRequest = Omit<Extract<ClientMessage, { type: 'join_room' }>, 'type' | 'client_protocol'>;

export interface Timers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

export interface RoomSocketOptions {
  /** `wss://host/ws?code=CODE`; `protocol=<PROTOCOL_VERSION>` is added. */
  url: string;
  join: JoinRequest;
  /** The creator's token from `POST /api/rooms`, or one saved from an earlier welcome. */
  resumeToken?: string | null;
  makeSocket?: (url: string) => WebSocket;
  now?: () => number;
  /** Fires `visibilitychange` and `pageshow` and carries `visibilityState`.
   *  Defaults to `document` + `window`. */
  visibility?: EventTarget;
  timers?: Timers;
  rand?: () => number;
  /** Called when an upgrade fails before opening (browsers hide the HTTP
   *  status): resolves to the status of an equivalent plain HTTP request, or
   *  null if unknown. Defaults to `httpProbe(url)` in a browser (the Worker
   *  sends the site's CORS header on its /ws refusals); null turns it off. */
  probe?: (() => Promise<number | null>) | null;
  /** Raw "ping" keepalive / health check (the Worker auto-answers "pong").
   *  Turn off against the local axum server, which answers it with an error. */
  keepalive?: boolean;
}

// ---- numbers -------------------------------------------------------------

/** Backoff: 1 s, 2 s, 4 s, 8 s, 16 s, then 30 s, each plus 0–25 % jitter. */
export const BACKOFF_BASE_MS = 1000;
export const BACKOFF_CAP_MS = 30_000;
export const BACKOFF_JITTER = 0.25;
/** An upgrade refused at the HTTP level (429/403 per the probe) waits at least
 *  attempt 3 of the curve (8 s), so with the probe a refused client makes at most
 *  4 attempts (8 requests) in its first minute against the Worker's 20/min
 *  per-IP lookup budget, then 2 a minute. */
export const REFUSED_MIN_ATTEMPT = 3;
/** Drag moves (`move_piece`, `move_selection`): at most 30 a second. */
export const DRAG_INTERVAL_MS = Math.ceil(1000 / 30);
/** Cursors (`cursor` + `cursor_hidden`): at most 15 a second (the Worker relays 30 per 2 s). */
export const CURSOR_INTERVAL_MS = Math.ceil(1000 / 15);
/** Keepalive when nothing else was sent: the Worker drops sockets silent for 90 s. */
export const HEARTBEAT_MS = 30_000;
/** A foreground health check that hears nothing back in this long replaces the socket. */
export const HEALTH_TIMEOUT_MS = 4000;
/** A connection attempt with no welcome after this long is dropped and retried on the backoff. */
export const WELCOME_TIMEOUT_MS = 12_000;

export function nextDelay(attempt: number, rand: () => number): number {
  const a = Number.isNaN(attempt) ? 0 : Math.max(0, Math.min(attempt, 30));
  const base = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** Math.floor(a));
  const r = rand();
  const j = Number.isFinite(r) ? Math.min(1, Math.max(0, r)) : 0;
  return Math.round(base * (1 + BACKOFF_JITTER * j));
}

// ---- server refusals -----------------------------------------------------

/** Map a server `error` text to the fatal reason it means, if any. The texts
 *  are core's (`claims.rs`, `moderation.rs`, `membership.rs`), the Worker's
 *  (`room.ts`, `index.ts`) and the axum server's protocol check. */
export function classifyError(message: string): FatalReason | null {
  if (message.startsWith('This name was opened in another window')) return 'opened-elsewhere';
  if (message.startsWith('That name is taken in this room')) return 'name-taken';
  if (message.startsWith('Pick a different name for public rooms')) return 'name-taken';
  if (message.startsWith('The host removed you from this room')) return 'banned';
  if (message === 'Room code not found') return 'room-gone';
  if (message.startsWith('PieceOff Online is paused')) return 'paused';
  if (/^This PieceOff app is newer/.test(message)) return 'update-server';
  if (/^This PieceOff server is newer/.test(message) || /Update PieceOff/.test(message)) return 'update-client';
  return null;
}

function classifyClose(code: number, reason: string): FatalReason | null {
  if (reason === 'replaced') return 'opened-elsewhere';
  if (reason === 'removed by host') return 'kicked';
  if (reason === 'Room code not found' || (code === 1001 && reason === 'room expired')) return 'room-gone';
  return null;
}

/** A probe for `RoomSocketOptions.probe`: the same URL over HTTP(S). The Worker
 *  answers 503 while paused, 429 when rate limited, 403 for a refused origin and
 *  426 otherwise. It counts against the per-IP lookup budget, so it only runs
 *  after a failed upgrade. */
export function httpProbe(wsUrl: string, fetchFn: typeof fetch = fetch): () => Promise<number | null> {
  const http = wsUrl.replace(/^ws(s?):/, 'http$1:');
  return async () => {
    try {
      return (await fetchFn(http, { method: 'GET', cache: 'no-store', redirect: 'error' })).status;
    } catch {
      return null;
    }
  };
}

// ---- the socket ----------------------------------------------------------

interface Slot {
  last: number;
  pending: ClientMessage | null;
  timer: unknown;
}

const OPEN = 1;

function remove<T>(list: T[], item: T): void {
  const i = list.indexOf(item);
  if (i >= 0) list.splice(i, 1);
}

export class RoomSocket {
  private readonly url: string;
  private readonly join: JoinRequest;
  private readonly makeSocket: (url: string) => WebSocket;
  private readonly now: () => number;
  private readonly timers: Timers;
  private readonly rand: () => number;
  private readonly probe?: () => Promise<number | null>;
  private readonly keepalive: boolean;
  private readonly isHidden: () => boolean;
  private readonly listen: () => void;
  private readonly unlisten: () => void;

  private ws: WebSocket | null = null;
  private wsOpened = false;
  private joined = false;
  private state: SocketState = 'connecting';
  private why: FatalReason | undefined;
  private stopped = false; // close() or fatal
  private attempt = 0;
  private lastAttemptAt = Number.NEGATIVE_INFINITY;
  private lastRefused = false;
  private retryTimer: unknown = null;
  private heartbeatTimer: unknown = null;
  private healthTimer: unknown = null;
  private welcomeTimer: unknown = null;
  private lastTx = 0;
  private lastRx = 0;
  private cycle = 0; // bumps on every connect so stale probe replies are ignored

  private readonly drag: Slot = { last: Number.NEGATIVE_INFINITY, pending: null, timer: null };
  private readonly cursor: Slot = { last: Number.NEGATIVE_INFINITY, pending: null, timer: null };
  private cursorAt: { x: number; y: number } | null = null; // latest requested
  private cursorShown = false;

  private readonly messageCbs: Array<(m: ServerMessage) => void> = [];
  private readonly stateCbs: Array<(s: SocketState, why?: FatalReason) => void> = [];

  /** The latest resume token (from the last welcome); save it to survive a refresh. */
  resumeToken: string | null;
  /** This connection's client id from the last welcome. */
  clientId: string | null = null;

  constructor(o: RoomSocketOptions) {
    const u = new URL(o.url);
    if (!u.searchParams.has('protocol')) u.searchParams.set('protocol', String(PROTOCOL_VERSION));
    this.url = u.toString();
    this.join = o.join;
    this.resumeToken = o.resumeToken || null;
    this.makeSocket = o.makeSocket ?? ((url) => new WebSocket(url));
    this.now = o.now ?? (() => Date.now());
    this.timers = o.timers ?? {
      set: (fn, ms) => setTimeout(fn, ms),
      clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
    };
    this.rand = o.rand ?? Math.random;
    this.probe =
      o.probe === undefined
        ? typeof window !== 'undefined' && typeof fetch === 'function'
          ? httpProbe(this.url)
          : undefined
        : (o.probe ?? undefined);
    this.keepalive = o.keepalive ?? true;

    const onVisibility = () => {
      if (!this.isHidden()) this.healthCheck();
    };
    let target: { doc: EventTarget; win: EventTarget } | null = null;
    if (o.visibility) {
      const v = o.visibility as EventTarget & { visibilityState?: string };
      this.isHidden = () => v.visibilityState === 'hidden';
      target = { doc: v, win: v };
    } else if (typeof document !== 'undefined' && typeof window !== 'undefined') {
      this.isHidden = () => document.visibilityState === 'hidden';
      target = { doc: document, win: window };
    } else {
      this.isHidden = () => false;
    }
    // Removed on close() and on a fatal stop (so a dropped socket does not stay
    // pinned by document/window listeners), added again by retry().
    this.listen = () => {
      target?.doc.addEventListener('visibilitychange', onVisibility);
      target?.win.addEventListener('pageshow', onVisibility);
    };
    this.unlisten = () => {
      target?.doc.removeEventListener('visibilitychange', onVisibility);
      target?.win.removeEventListener('pageshow', onVisibility);
    };
    this.listen();
    this.connect();
  }

  /** Server messages, from the accepted welcome on. A welcome from a server on
   *  another protocol is never delivered. Raw "pong" frames are swallowed. */
  onMessage(cb: (m: ServerMessage) => void): () => void {
    this.messageCbs.push(cb);
    return () => void remove(this.messageCbs, cb);
  }

  /** Called at once with the current state, then on every change.
   *  `why` is set for 'fatal', and is 'rate-limited' on a 'reconnecting'
   *  that follows a 429 (the socket keeps retrying on the backoff). */
  onState(cb: (s: SocketState, why?: FatalReason) => void): () => void {
    this.stateCbs.push(cb);
    cb(this.state, this.why);
    return () => void remove(this.stateCbs, cb);
  }

  get current(): { state: SocketState; why?: FatalReason } {
    return { state: this.state, why: this.why };
  }

  /**
   * Send an intent. Returns false when it was dropped: nothing is queued while
   * the room is not open (before the welcome, while reconnecting, after a
   * fatal), because a reconnect brings a fresh snapshot and old intents would
   * act on a board that no longer exists. Drag moves are coalesced to ≤30/s
   * with a trailing update; cursors to ≤15/s and only when they moved.
   */
  send(m: ClientMessage): boolean {
    if (!this.joined || !this.ws) return false;
    switch (m.type) {
      case 'move_piece':
      case 'move_selection':
        this.throttle(this.drag, m, DRAG_INTERVAL_MS);
        return true;
      case 'cursor': {
        const at = this.cursorAt;
        if (at && at.x === m.x && at.y === m.y) return false;
        this.cursorAt = { x: m.x, y: m.y };
        this.throttle(this.cursor, m, CURSOR_INTERVAL_MS);
        return true;
      }
      case 'cursor_hidden':
        this.cancel(this.cursor);
        this.cursorAt = null;
        if (!this.cursorShown) return false;
        this.cursorShown = false;
        this.write(m);
        return true;
      case 'drag_end':
      case 'drag_end_selection':
        this.cancel(this.drag); // the end carries the final position
        this.write(m);
        return true;
      default:
        this.flush(this.drag); // keep intent order: the move happened first
        this.write(m);
        return true;
    }
  }

  /**
   * Check the connection now (also run on `visibilitychange`/`pageshow`):
   * a connection attempt in flight is left alone; a pending retry runs at
   * once (unless the last refusal was a 429/403, or one ran under a second
   * ago after repeated failures); an open socket gets a raw "ping" and is
   * replaced if nothing comes back within HEALTH_TIMEOUT_MS.
   */
  healthCheck(): void {
    if (this.stopped) return;
    if (this.ws && !this.joined) return; // attempt in flight
    if (!this.ws) {
      if (this.retryTimer === null || this.lastRefused) return;
      if (this.attempt > 1 && this.now() - this.lastAttemptAt < BACKOFF_BASE_MS) return;
      this.connect();
      return;
    }
    if (this.ws.readyState !== OPEN) {
      this.replaceNow();
      return;
    }
    if (!this.keepalive || this.healthTimer !== null) return;
    const asked = this.now();
    this.writeRaw('ping');
    this.healthTimer = this.timers.set(() => {
      this.healthTimer = null;
      if (this.lastRx < asked) this.replaceNow();
    }, HEALTH_TIMEOUT_MS);
  }

  /** Try again after a fatal stop (a "Try again" button; e.g. after 'paused'). */
  retry(): void {
    if (this.state !== 'fatal') return;
    this.stopped = false;
    this.why = undefined;
    this.attempt = 0;
    this.lastRefused = false;
    this.listen();
    this.setState('connecting');
    this.connect();
  }

  close(): void {
    this.stopped = true;
    this.teardown();
    this.unlisten();
  }

  // ---- internals ----

  private connect(): void {
    this.clearTimer('retryTimer');
    this.cycle++;
    this.lastAttemptAt = this.now();
    this.wsOpened = false;
    const ws = this.makeSocket(this.url);
    this.ws = ws;
    // A CONNECTING socket can stall for minutes (captive portals, a half-dead
    // radio) with no close event: give up on it and back off like a drop.
    this.welcomeTimer = this.timers.set(() => {
      this.welcomeTimer = null;
      if (ws !== this.ws || this.joined) return;
      this.detach();
      try {
        ws.close(4000, 'timeout');
      } catch {
        // already gone
      }
      this.lastRefused = false;
      this.scheduleRetry(undefined);
    }, WELCOME_TIMEOUT_MS);
    ws.onopen = () => {
      if (ws !== this.ws) return;
      this.wsOpened = true;
      const join: Extract<ClientMessage, { type: 'join_room' }> & { resume_token?: string } = {
        type: 'join_room',
        ...this.join,
        client_protocol: PROTOCOL_VERSION,
      };
      if (this.resumeToken) join.resume_token = this.resumeToken;
      this.writeRaw(JSON.stringify(join));
    };
    ws.onmessage = (e: MessageEvent) => {
      if (ws !== this.ws) return;
      this.lastRx = this.now();
      this.receive(e.data);
    };
    ws.onclose = (e: CloseEvent) => {
      if (ws !== this.ws) return;
      this.dropped(e.code, e.reason ?? '');
    };
    ws.onerror = () => {
      // a close event follows
    };
  }

  private receive(data: unknown): void {
    if (typeof data !== 'string' || data === 'pong') return;
    let m: (ServerMessage & { resume_token?: unknown }) | null = null;
    try {
      m = JSON.parse(data);
    } catch {
      return;
    }
    if (!m || typeof m !== 'object' || typeof m.type !== 'string') return;
    switch (m.type) {
      case 'welcome': {
        const check = checkServerProtocol(m.snapshot?.protocol_version, PROTOCOL_VERSION);
        if (check !== 'ok') {
          this.fatal(check);
          return;
        }
        const token = m.resume_token;
        if (typeof token === 'string' && token.length > 0 && token.length <= 256) this.resumeToken = token;
        this.clientId = m.client_id;
        this.joined = true;
        this.clearTimer('welcomeTimer');
        this.attempt = 0;
        this.lastRefused = false;
        this.armHeartbeat();
        this.setState('open');
        this.emit(m);
        return;
      }
      case 'error': {
        if (typeof m.message !== 'string') return; // malformed: nothing to show or classify
        this.emit(m);
        const reason = classifyError(m.message);
        // Join refusals (name taken, public name, banned, update, paused) are
        // only fatal before the welcome: after it the same texts answer room
        // changes (e.g. set_listed refuses a host's blocked name with
        // "Pick a different name for public rooms") and are ordinary errors.
        if (reason && (!this.joined || reason === 'opened-elsewhere' || reason === 'room-gone')) {
          this.fatal(reason);
        }
        return;
      }
      case 'kicked':
        if (!this.joined) return;
        this.emit(m);
        if (m.client_id === this.clientId) this.fatal('kicked');
        return;
      default:
        if (this.joined) this.emit(m);
    }
  }

  /** The socket closed (or was found dead): fatal, or back off and retry. */
  private dropped(code: number, reason: string): void {
    const opened = this.wsOpened;
    this.detach();
    const fatal = classifyClose(code, reason);
    if (fatal) {
      this.fatal(fatal);
      return;
    }
    if (opened || !this.probe) {
      this.lastRefused = false;
      this.scheduleRetry(undefined);
      return;
    }
    // Refused before opening: ask over HTTP why (the browser will not say).
    const cycle = this.cycle;
    this.setState('reconnecting');
    this.probe().then(
      (status) => this.probed(cycle, status),
      () => this.probed(cycle, null),
    );
  }

  private probed(cycle: number, status: number | null): void {
    if (this.stopped || cycle !== this.cycle || this.ws) return;
    if (status === 503) return this.fatal('paused');
    if (status === 429 || status === 403) {
      this.lastRefused = true;
      this.attempt = Math.max(this.attempt, REFUSED_MIN_ATTEMPT);
      this.scheduleRetry(status === 429 ? 'rate-limited' : undefined);
      return;
    }
    this.lastRefused = false;
    this.scheduleRetry(undefined);
  }

  private scheduleRetry(why: FatalReason | undefined): void {
    if (this.stopped) return;
    const delay = nextDelay(this.attempt, this.rand);
    this.attempt++;
    this.clearTimer('retryTimer');
    this.retryTimer = this.timers.set(() => {
      this.retryTimer = null;
      if (!this.stopped && !this.ws) this.connect();
    }, delay);
    this.setState('reconnecting', why);
  }

  /** Drop a dead socket ourselves and connect again immediately. */
  private replaceNow(): void {
    const ws = this.ws;
    this.detach();
    try {
      ws?.close(4000, 'stale');
    } catch {
      // already gone
    }
    this.setState('reconnecting');
    this.connect();
  }

  private fatal(why: FatalReason): void {
    if (this.stopped) return;
    this.stopped = true;
    this.teardown();
    this.unlisten();
    this.setState('fatal', why);
  }

  /** Forget the socket: its later events are ignored; throttled sends are dropped. */
  private detach(): void {
    const ws = this.ws;
    this.ws = null;
    this.joined = false;
    this.wsOpened = false;
    if (ws) ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
    this.cancel(this.drag);
    this.cancel(this.cursor);
    this.cursorAt = null;
    this.cursorShown = false;
    this.clearTimer('heartbeatTimer');
    this.clearTimer('healthTimer');
    this.clearTimer('welcomeTimer');
  }

  private teardown(): void {
    const ws = this.ws;
    this.detach();
    this.clearTimer('retryTimer');
    this.cycle++;
    try {
      ws?.close(1000, 'closed');
    } catch {
      // already gone
    }
  }

  private armHeartbeat(): void {
    this.clearTimer('heartbeatTimer');
    if (!this.keepalive) return;
    this.heartbeatTimer = this.timers.set(() => {
      this.heartbeatTimer = null;
      if (!this.joined) return;
      if (this.now() - this.lastTx >= HEARTBEAT_MS) this.writeRaw('ping');
      this.armHeartbeat();
    }, HEARTBEAT_MS);
  }

  private throttle(slot: Slot, m: ClientMessage, interval: number): void {
    if (slot.timer !== null) {
      slot.pending = m;
      return;
    }
    const t = this.now();
    if (t - slot.last >= interval) {
      slot.last = t;
      this.write(m);
      return;
    }
    slot.pending = m;
    slot.timer = this.timers.set(() => {
      slot.timer = null;
      const next = slot.pending;
      slot.pending = null;
      if (next && this.joined) {
        slot.last = this.now();
        this.write(next);
      }
    }, slot.last + interval - t);
  }

  private flush(slot: Slot): void {
    const next = slot.pending;
    this.cancel(slot);
    if (next) {
      slot.last = this.now();
      this.write(next);
    }
  }

  private cancel(slot: Slot): void {
    if (slot.timer !== null) this.timers.clear(slot.timer);
    slot.timer = null;
    slot.pending = null;
  }

  private write(m: ClientMessage): void {
    if (m.type === 'cursor') this.cursorShown = true;
    this.writeRaw(JSON.stringify(m));
  }

  private writeRaw(data: string): void {
    const ws = this.ws;
    if (!ws || ws.readyState !== OPEN) return;
    try {
      ws.send(data);
      this.lastTx = this.now();
    } catch {
      // the close event will follow
    }
  }

  private emit(m: ServerMessage): void {
    for (const cb of [...this.messageCbs]) cb(m);
  }

  private setState(s: SocketState, why?: FatalReason): void {
    if (s === this.state && why === this.why) return;
    this.state = s;
    this.why = why;
    for (const cb of [...this.stateCbs]) cb(s, why);
  }

  private clearTimer(k: 'retryTimer' | 'heartbeatTimer' | 'healthTimer' | 'welcomeTimer'): void {
    if (this[k] !== null) this.timers.clear(this[k]);
    this[k] = null;
  }
}
