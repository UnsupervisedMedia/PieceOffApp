// The public lobby: `GET /api/lobby` (the Worker caches it 5 s and allows a
// request per client every few seconds), polled every 10 s like desktop, only
// while the page is visible. Entries are validated and tidied the way the
// desktop client does (crates/pieceoff-app/src/online.rs parse_lobby).
import { public_name_allowed } from '../../wasm/pieceoff_web_wasm.js';
import { parseCode } from './join';

export const LOBBY_POLL_MS = 10_000;
/** After a 429 the next look waits this long instead. */
export const LOBBY_BACKOFF_MS = 60_000;
const MAX_ROOMS = 100;

export interface LobbyRoom {
  code: string;
  /** The built-in loop's bundled title (the Worker never lists client text here). */
  title: string;
  /** Blank when the public-name filter refuses it. */
  host_name: string;
  players: number;
  max_players: number;
  spectators: number;
  pieces: number;
  placed: number;
  cut_style: string;
  created_at: number;
}

export type LobbyErrorKind = 'rate-limited' | 'paused' | 'network' | 'bad-response' | 'server';

export class LobbyError extends Error {
  constructor(
    readonly kind: LobbyErrorKind,
    readonly status: number | null,
  ) {
    super(
      {
        'rate-limited': 'The room list is busy. Trying again in a minute.',
        paused: 'PieceOff Online is paused right now.',
        network: 'Could not reach PieceOff Online. Checking again shortly.',
        'bad-response': 'PieceOff Online sent an unreadable room list.',
        server: 'The room list is not answering right now.',
      }[kind],
    );
    this.name = 'LobbyError';
  }
}

const CONTROL = /\p{Cc}/gu;
const text = (v: unknown, max: number): string =>
  typeof v === 'string' ? [...v.replace(CONTROL, '')].slice(0, max).join('').trim() : '';
const count = (v: unknown, cap: number): number | null =>
  typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 ? Math.min(v, cap) : null;
const CUTS = new Set(['jigsaw', 'hexagon', 'triangle']);

function clean(raw: unknown, allowed: (n: string) => boolean): LobbyRoom | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  const code = parseCode(typeof r.code === 'string' ? r.code : null);
  const players = count(r.players, 999);
  const max = count(r.max_players, 999);
  if (!code || players === null || max === null || max === 0) return null;
  const pieces = count(r.pieces, 100_000) ?? 0;
  const host = text(r.host_name, 24);
  return {
    code,
    title: text(r.title, 80) || 'Puzzle',
    host_name: allowed(host) ? host : '',
    players,
    max_players: max,
    spectators: count(r.spectators, 9_999) ?? 0,
    pieces,
    placed: Math.min(count(r.placed, 100_000) ?? 0, pieces),
    cut_style: typeof r.cut_style === 'string' && CUTS.has(r.cut_style) ? r.cut_style : 'jigsaw',
    created_at: typeof r.created_at === 'number' && Number.isFinite(r.created_at) ? r.created_at : 0,
  };
}

export const hasSeat = (r: LobbyRoom): boolean => r.players < r.max_players;

/** Desktop's lobby order: a free seat first, then the busiest, then the newest, then by code. */
export function lobbyOrder(a: LobbyRoom, b: LobbyRoom): number {
  return (
    Number(hasSeat(b)) - Number(hasSeat(a)) ||
    b.players - a.players ||
    b.created_at - a.created_at ||
    (a.code < b.code ? -1 : a.code > b.code ? 1 : 0)
  );
}

/** The room Quick match joins: the first in lobby order with a seat, or null (start one instead). */
export function quickMatch(rooms: LobbyRoom[]): LobbyRoom | null {
  let best: LobbyRoom | null = null;
  for (const r of rooms) if (hasSeat(r) && (best === null || lobbyOrder(r, best) < 0)) best = r;
  return best;
}

export interface LobbyFetchOptions {
  fetch?: typeof fetch;
  publicNameAllowed?: (name: string) => boolean;
}

export async function fetchLobby(api: string, o: LobbyFetchOptions = {}): Promise<LobbyRoom[]> {
  const f = o.fetch ?? fetch;
  const allowed = o.publicNameAllowed ?? ((n: string) => n === '' || public_name_allowed(n));
  let res: Response;
  try {
    res = await f(`${api}/api/lobby`, { method: 'GET', credentials: 'omit', cache: 'no-store', redirect: 'error' });
  } catch {
    throw new LobbyError('network', null);
  }
  if (res.status === 429) throw new LobbyError('rate-limited', 429);
  if (res.status === 503) throw new LobbyError('paused', 503);
  if (!res.ok) throw new LobbyError('server', res.status);
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new LobbyError('bad-response', res.status);
  }
  const list = body && typeof body === 'object' ? (body as { rooms?: unknown }).rooms : undefined;
  if (!Array.isArray(list)) throw new LobbyError('bad-response', res.status);
  const seen = new Set<string>();
  const rooms: LobbyRoom[] = [];
  for (const raw of list.slice(0, MAX_ROOMS * 2)) {
    const r = clean(raw, allowed);
    if (r && !seen.has(r.code)) {
      seen.add(r.code);
      rooms.push(r);
    }
  }
  return rooms.sort(lobbyOrder).slice(0, MAX_ROOMS);
}

export interface LobbyUpdate {
  rooms?: LobbyRoom[];
  error?: LobbyError;
}

export interface LobbyPollerOptions extends LobbyFetchOptions {
  api: string;
  /** Fires `visibilitychange`/`pageshow` and carries `visibilityState`. Defaults to `document`. */
  visibility?: EventTarget & { visibilityState?: string };
}

/** Polls the lobby every 10 s while running and the page is visible. */
export class LobbyPoller {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private running = false;
  private inflight = false;
  private readonly cbs: Array<(u: LobbyUpdate) => void> = [];
  private readonly vis: EventTarget & { visibilityState?: string };
  private readonly onVisibility = () => {
    if (!this.running) return;
    if (this.hidden()) this.clear();
    else if (this.timer === null && !this.inflight) void this.load();
  };

  constructor(private readonly o: LobbyPollerOptions) {
    this.vis = o.visibility ?? (typeof document !== 'undefined' ? document : new EventTarget());
  }

  onUpdate(cb: (u: LobbyUpdate) => void): () => void {
    this.cbs.push(cb);
    return () => {
      const i = this.cbs.indexOf(cb);
      if (i >= 0) this.cbs.splice(i, 1);
    };
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.vis.addEventListener('visibilitychange', this.onVisibility);
    this.vis.addEventListener('pageshow', this.onVisibility);
    if (!this.hidden()) void this.load();
  }

  stop(): void {
    this.running = false;
    this.clear();
    this.vis.removeEventListener('visibilitychange', this.onVisibility);
    this.vis.removeEventListener('pageshow', this.onVisibility);
  }

  /** Look now (a Refresh button), unless a look is already in flight. */
  refresh(): void {
    if (!this.running || this.inflight) return;
    this.clear();
    void this.load();
  }

  private hidden(): boolean {
    return this.vis.visibilityState === 'hidden';
  }

  private clear(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  private async load(): Promise<void> {
    this.inflight = true;
    let wait = LOBBY_POLL_MS;
    let update: LobbyUpdate;
    try {
      update = { rooms: await fetchLobby(this.o.api, this.o) };
    } catch (e) {
      const err = e instanceof LobbyError ? e : new LobbyError('network', null);
      if (err.kind === 'rate-limited') wait = LOBBY_BACKOFF_MS;
      update = { error: err };
    } finally {
      this.inflight = false;
    }
    if (!this.running) return;
    for (const cb of [...this.cbs]) cb(update);
    this.clear();
    if (!this.hidden()) this.timer = setTimeout(() => void this.load(), wait);
  }
}
