// @vitest-environment node
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { protocol_version } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import type { ClientMessage } from '../protocol';
import {
  BACKOFF_BASE_MS,
  BACKOFF_CAP_MS,
  BACKOFF_JITTER,
  CURSOR_INTERVAL_MS,
  DRAG_INTERVAL_MS,
  HEALTH_TIMEOUT_MS,
  HEARTBEAT_MS,
  REFUSED_MIN_ATTEMPT,
  WELCOME_TIMEOUT_MS,
  RoomSocket,
  classifyError,
  httpProbe,
  nextDelay,
  type FatalReason,
  type RoomSocketOptions,
  type SocketState,
} from './socket';
import { FakeSocket, FakeVisibility, welcome } from './testkit';

let V: number;
beforeAll(async () => {
  await initWasm();
  V = protocol_version();
});

beforeEach(() => {
  vi.useFakeTimers({ now: 1_000_000 });
});
afterEach(() => {
  vi.useRealTimers();
});

const JOIN = {
  invite_code: 'ABCD2345',
  name: 'Ann',
  media_sha256: 'a'.repeat(64),
  media_extension: 'webp',
  spectator: false,
  player_key: 'k'.repeat(32),
};

// Core strings, verbatim (crates/pieceoff-core claims.rs / moderation.rs, worker/src/room.ts).
const NAME_TAKEN =
  'That name is taken in this room. Pick another name, or ask the host to release it (names free up 10 minutes after their player leaves).';
const REPLACED_CORE =
  'This name was opened in another window or on another device using the same settings, so this window was disconnected.';
const REPLACED_WORKER = 'This name was opened in another window or on another device using the same settings';
const REMOVED_BY_HOST = 'The host removed you from this room.';
const PUBLIC_NAME_REFUSED = 'Pick a different name for public rooms';

function setup(extra: Partial<RoomSocketOptions> = {}) {
  const sockets: FakeSocket[] = [];
  const created: number[] = [];
  const vis = new FakeVisibility();
  const rs = new RoomSocket({
    url: 'wss://rooms.test/ws?code=ABCD2345',
    join: JOIN,
    makeSocket: (u) => {
      const s = new FakeSocket(u);
      sockets.push(s);
      created.push(Date.now());
      return s as unknown as WebSocket;
    },
    now: () => Date.now(),
    visibility: vis,
    rand: () => 0,
    ...extra,
  });
  const states: Array<[SocketState, FatalReason | undefined]> = [];
  rs.onState((s, why) => states.push([s, why]));
  const msgs: Array<{ type: string }> = [];
  rs.onMessage((m) => msgs.push(m));
  const last = () => sockets[sockets.length - 1]!;
  const join = (token = 'token-1', over = {}) => {
    last().open();
    last().receive(welcome(V, over, token));
  };
  return { rs, sockets, created, vis, states, msgs, last, join };
}

const state = (states: Array<[SocketState, FatalReason | undefined]>) => states[states.length - 1];

describe('nextDelay', () => {
  it('backoff_curve_doubles_from_1s_caps_at_30s_and_jitters_up_to_25_percent', () => {
    expect(BACKOFF_BASE_MS).toBe(1000);
    expect(BACKOFF_CAP_MS).toBe(30_000);
    expect(BACKOFF_JITTER).toBe(0.25);
    let prevLow = 0;
    for (let a = 0; a <= 40; a++) {
      const low = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** a);
      expect(nextDelay(a, () => 0)).toBe(low);
      expect(nextDelay(a, () => 0.5)).toBe(Math.round(low * 1.125));
      const high = nextDelay(a, () => 0.999999);
      expect(high).toBeLessThanOrEqual(low * 1.25);
      expect(high).toBeGreaterThan(low);
      expect(low).toBeGreaterThanOrEqual(prevLow);
      prevLow = low;
    }
    expect(nextDelay(5, () => 0)).toBe(BACKOFF_CAP_MS);
    expect(nextDelay(4, () => 0)).toBe(16_000);
  });

  it('treats bad attempts and bad random values safely', () => {
    expect(nextDelay(-3, () => 0)).toBe(1000);
    expect(nextDelay(Number.NaN, () => 0)).toBe(1000);
    expect(nextDelay(Number.POSITIVE_INFINITY, () => 0)).toBe(BACKOFF_CAP_MS);
    expect(nextDelay(0, () => 7)).toBe(1250);
    expect(nextDelay(0, () => -1)).toBe(1000);
    expect(nextDelay(0, () => Number.NaN)).toBe(1000);
  });
});

describe('RoomSocket handshake', () => {
  it('opens /ws with the generated protocol version and joins with player_key and client_protocol', () => {
    const { last, states } = setup({ resumeToken: 'creator-token' });
    expect(state(states)).toEqual(['connecting', undefined]);
    const url = new URL(last().url);
    expect(url.searchParams.get('code')).toBe('ABCD2345');
    expect(url.searchParams.get('protocol')).toBe(String(V));
    last().open();
    const [join] = last().ofType('join_room');
    expect(join).toEqual({
      type: 'join_room',
      ...JOIN,
      resume_token: 'creator-token',
      client_protocol: V,
    });
    expect(last().frames).toHaveLength(1);
  });

  it('fatal_update_client_when_server_protocol_newer', () => {
    const { rs, last, sockets, states, msgs } = setup();
    last().open();
    last().receive(welcome(V + 1));
    expect(state(states)).toEqual(['fatal', 'update-client']);
    // No half-working state: the welcome never reaches the app, the socket is
    // closed, nothing can be sent and nothing reconnects.
    expect(msgs.map((m) => m.type)).not.toContain('welcome');
    expect(last().readyState).toBe(FakeSocket.CLOSED);
    expect(rs.send({ type: 'chat', text: 'hi' })).toBe(false);
    vi.advanceTimersByTime(10 * 60_000);
    rs.healthCheck();
    expect(sockets).toHaveLength(1);
    expect(states.filter(([s]) => s === 'open')).toHaveLength(0);
  });

  it('an older server is update-server, also fatal', () => {
    const { last, sockets, states } = setup();
    last().open();
    last().receive(welcome(V - 1));
    expect(state(states)).toEqual(['fatal', 'update-server']);
    vi.advanceTimersByTime(10 * 60_000);
    expect(sockets).toHaveLength(1);
  });

  it('a version refusal sent as an error (worker or axum) is fatal update-client', () => {
    for (const message of [
      'This room turns its pieces. Update PieceOff to join it.',
      `This PieceOff server is newer (app protocol v${V}, server protocol v${V + 1}). Update so the app and the room server match.`,
    ]) {
      const { last, sockets, states, msgs } = setup();
      last().open();
      last().receive({ type: 'error', message });
      last().serverClose(1008, 'join refused');
      expect(state(states)).toEqual(['fatal', 'update-client']);
      expect(msgs).toEqual([{ type: 'error', message }]);
      vi.advanceTimersByTime(10 * 60_000);
      expect(sockets).toHaveLength(1);
    }
  });

  it('welcome opens the socket, hands the welcome on and keeps the fresh resume token', () => {
    const { rs, join, states, msgs } = setup();
    join('fresh');
    expect(state(states)).toEqual(['open', undefined]);
    expect(msgs.map((m) => m.type)).toEqual(['welcome']);
    expect(rs.resumeToken).toBe('fresh');
    expect(rs.clientId).toBe('me');
  });

  it('ignores raw pong frames and non-JSON noise', () => {
    const { join, last, msgs } = setup();
    join();
    last().receive('pong');
    last().receive('{not json');
    last().receive('42');
    expect(msgs.map((m) => m.type)).toEqual(['welcome']);
  });
});

describe('RoomSocket reconnect', () => {
  it('reconnect_after_close_resumes_with_same_player_key_and_resume_token', () => {
    const { join, last, sockets, states } = setup({ resumeToken: 'creator-token' });
    join('fresh-1');
    last().serverClose(1006);
    expect(state(states)).toEqual(['reconnecting', undefined]);
    const first = nextDelay(0, () => 0);
    vi.advanceTimersByTime(first - 1);
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(2);
    last().open();
    const joins = last().ofType('join_room');
    expect(joins).toHaveLength(1);
    // Same identity: the token from the last welcome (not the stale creator
    // one) and the same player key, so the server adopts the same client id
    // and no ghost player is left behind.
    expect(joins[0]).toMatchObject({ resume_token: 'fresh-1', player_key: JOIN.player_key, name: 'Ann' });
    last().receive(welcome(V, {}, 'fresh-2'));
    expect(state(states)).toEqual(['open', undefined]);
    // The attempt counter resets on welcome: the next drop waits the first step again.
    last().serverClose(1006);
    vi.advanceTimersByTime(first);
    expect(sockets).toHaveLength(3);
    last().open();
    expect(last().ofType('join_room')[0]).toMatchObject({ resume_token: 'fresh-2' });
  });

  it('opening without a welcome does not reset the backoff (join refusals back off too)', () => {
    const { last, created } = setup();
    for (let i = 0; i < 5; i++) {
      last().open();
      last().receive({ type: 'error', message: 'That player is already connected' });
      last().serverClose(1008, 'already connected');
      vi.advanceTimersByTime(nextDelay(i, () => 0));
    }
    const gaps = created.slice(1).map((t, i) => t - created[i]!);
    expect(gaps).toEqual([0, 1, 2, 3, 4].map((i) => nextDelay(i, () => 0)));
  });

  it('background_then_foreground_reconnects_once_no_duplicate_join', () => {
    const { join, last, sockets, vis, states } = setup();
    join('fresh-1');
    vis.hide();
    // The OS kills the socket while the tab is in the background.
    last().serverClose(1006);
    expect(state(states)).toEqual(['reconnecting', undefined]);
    vi.advanceTimersByTime(200); // timers are throttled while hidden; the retry has not fired
    expect(sockets).toHaveLength(1);
    vis.show(); // visibilitychange + pageshow
    expect(sockets).toHaveLength(2); // immediately, once, not after the backoff
    vis.show();
    vis.dispatchEvent(new Event('pageshow'));
    expect(sockets).toHaveLength(2); // an attempt in flight is never doubled
    last().open();
    vis.show();
    last().receive(welcome(V, {}, 'fresh-2'));
    vi.advanceTimersByTime(5 * 60_000);
    expect(sockets).toHaveLength(2);
    expect(last().ofType('join_room')).toHaveLength(1);
    expect(last().ofType('join_room')[0]).toMatchObject({ resume_token: 'fresh-1', player_key: JOIN.player_key });
    expect(sockets[0]!.ofType('join_room')).toHaveLength(1);
    expect(state(states)).toEqual(['open', undefined]);
  });

  it('foreground health check replaces a silently dead socket, once', () => {
    const { join, last, sockets, vis, states } = setup();
    join();
    vis.hide();
    vi.advanceTimersByTime(2 * 60_000);
    const dead = last();
    const before = dead.frames.length;
    vis.show();
    // One probe for both events, a raw "ping" the Worker answers with "pong".
    expect(dead.frames.slice(before).map((f) => f.data)).toEqual(['ping']);
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(HEALTH_TIMEOUT_MS);
    expect(dead.readyState).toBe(FakeSocket.CLOSED);
    expect(sockets).toHaveLength(2);
    expect(state(states)).toEqual(['reconnecting', undefined]);
    vis.show();
    expect(sockets).toHaveLength(2);
    last().open();
    expect(last().ofType('join_room')).toHaveLength(1);
  });

  it('a live socket that answers the health check is kept', () => {
    const { join, last, sockets, vis, states } = setup();
    join();
    vis.hide();
    vis.show();
    last().receive('pong');
    vi.advanceTimersByTime(HEALTH_TIMEOUT_MS * 3);
    expect(sockets).toHaveLength(1);
    expect(state(states)).toEqual(['open', undefined]);
  });

  it('a socket the browser already closed is replaced at once on foreground', () => {
    const { join, last, sockets, vis } = setup();
    join();
    vis.hide();
    last().readyState = FakeSocket.CLOSED; // dead, close event not delivered yet
    vis.show();
    expect(sockets).toHaveLength(2);
  });

  it('sends a keepalive ping when idle so the Worker does not sweep the socket', () => {
    const { join, last, msgs } = setup();
    join();
    vi.advanceTimersByTime(HEARTBEAT_MS);
    expect(last().frames.map((f) => f.data)).toContain('ping');
    last().receive('pong');
    expect(msgs.map((m) => m.type)).toEqual(['welcome']);
  });

  it('no_reconnect_after_kicked_or_opened_elsewhere', () => {
    {
      const { join, last, sockets, states, msgs, vis } = setup();
      join();
      last().receive({ type: 'kicked', client_id: 'me', message: REMOVED_BY_HOST });
      last().serverClose(1008, 'removed by host');
      expect(state(states)).toEqual(['fatal', 'kicked']);
      expect(msgs.map((m) => m.type)).toEqual(['welcome', 'kicked']);
      vis.show();
      vi.advanceTimersByTime(60 * 60_000);
      expect(sockets).toHaveLength(1);
    }
    {
      const { join, last, sockets, states, vis } = setup();
      join();
      last().receive({ type: 'kicked', client_id: 'someone-else', message: REMOVED_BY_HOST });
      expect(state(states)).toEqual(['open', undefined]);
      for (const message of [REPLACED_WORKER, REPLACED_CORE]) {
        last().receive({ type: 'error', message });
      }
      last().serverClose(1008, 'replaced');
      expect(state(states)).toEqual(['fatal', 'opened-elsewhere']);
      vis.show();
      vi.advanceTimersByTime(60 * 60_000);
      expect(sockets).toHaveLength(1);
    }
    {
      // The close reason alone is enough if the error frame was lost.
      const { join, last, sockets, states } = setup();
      join();
      last().serverClose(1008, 'replaced');
      expect(state(states)).toEqual(['fatal', 'opened-elsewhere']);
      vi.advanceTimersByTime(60 * 60_000);
      expect(sockets).toHaveLength(1);
    }
  });

  it('name taken, banned and room gone never reconnect', () => {
    const cases: Array<[string, string, FatalReason]> = [
      [NAME_TAKEN, 'join refused', 'name-taken'],
      [PUBLIC_NAME_REFUSED, 'join refused', 'name-taken'],
      [REMOVED_BY_HOST, 'join refused', 'banned'],
      ['Room code not found', 'Room code not found', 'room-gone'],
    ];
    for (const [message, reason, why] of cases) {
      const { last, sockets, states } = setup();
      last().open();
      last().receive({ type: 'error', message });
      last().serverClose(1008, reason);
      expect(state(states)).toEqual(['fatal', why]);
      vi.advanceTimersByTime(60 * 60_000);
      expect(sockets).toHaveLength(1);
    }
    // An expiring room closes with 1001 "room expired".
    const { join, last, sockets, states } = setup();
    join();
    last().serverClose(1001, 'room expired');
    expect(state(states)).toEqual(['fatal', 'room-gone']);
    vi.advanceTimersByTime(60 * 60_000);
    expect(sockets).toHaveLength(1);
  });

  it('a join-refusal text after the welcome is an ordinary error (host listing with a blocked name)', () => {
    for (const message of [PUBLIC_NAME_REFUSED, NAME_TAKEN, REMOVED_BY_HOST, 'This room turns its pieces. Update PieceOff to join it.']) {
      const { rs, join, last, sockets, states, msgs } = setup();
      join();
      last().receive({ type: 'error', message });
      expect(state(states)).toEqual(['open', undefined]);
      expect(states.some(([s]) => s === 'fatal')).toBe(false);
      expect(msgs[msgs.length - 1]).toEqual({ type: 'error', message });
      expect(last().readyState).toBe(FakeSocket.OPEN);
      expect(rs.send({ type: 'chat', text: 'still here' })).toBe(true);
      vi.advanceTimersByTime(60_000);
      expect(sockets).toHaveLength(1);
    }
  });

  it('classifyError leaves ordinary errors alone', () => {
    expect(classifyError('You are sending chat messages too quickly. Wait a moment.')).toBeNull();
    expect(classifyError("That piece is in Bo's holder")).toBeNull();
    expect(classifyError('That player is already connected')).toBeNull();
    expect(classifyError('PieceOff Online is paused right now. Please try again later.')).toBe('paused');
  });

  it('429_and_403_do_not_retry_in_a_tight_loop (given the probe reads 429/403)', async () => {
    let n = 0;
    const statuses = [429, 403];
    const probe = vi.fn(async () => statuses[n++ % 2]!);
    let seed = 1;
    const rand = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    const { sockets, created, states, rs } = setup({ probe, rand });
    // Every upgrade is refused at the HTTP level (browser: error + close 1006).
    const refuseAll = () => {
      for (const s of sockets) if (s.readyState === FakeSocket.CONNECTING) s.refuse();
    };
    refuseAll();
    for (let t = 0; t < 10 * 60_000; t += 250) {
      await vi.advanceTimersByTimeAsync(250);
      refuseAll();
    }
    const gaps = created.slice(1).map((t, i) => t - created[i]!);
    expect(gaps.length).toBeGreaterThan(5);
    expect(REFUSED_MIN_ATTEMPT).toBe(3);
    expect(Math.min(...gaps)).toBeGreaterThanOrEqual(8000);
    gaps.forEach((gap, i) => {
      // Spaced by at least the capped backoff floor for that attempt, never
      // below the refusal floor (attempt REFUSED_MIN_ATTEMPT = 8 s).
      expect(gap).toBeGreaterThanOrEqual(nextDelay(Math.max(i, REFUSED_MIN_ATTEMPT), () => 0));
      expect(gap).toBeLessThanOrEqual(BACKOFF_CAP_MS * (1 + BACKOFF_JITTER) + 250);
    });
    expect(gaps.slice(-5).every((g) => g >= BACKOFF_CAP_MS)).toBe(true);
    // Well inside the Worker's per-IP lookup budget (20/min), probes included.
    for (const start of created) {
      const inMinute = created.filter((t) => t >= start && t < start + 60_000).length;
      expect(inMinute * 2).toBeLessThanOrEqual(20);
    }
    expect(states.some(([s, why]) => s === 'reconnecting' && why === 'rate-limited')).toBe(true);
    expect(states.some(([s]) => s === 'fatal')).toBe(false);
    rs.close();
  });

  it('a probe whose fetch is blocked (CORS TypeError) falls back to plain backoff, no label', async () => {
    const fetchFn = vi.fn(async () => {
      throw new TypeError('Failed to fetch');
    }) as unknown as typeof fetch;
    const { sockets, created, states } = setup({
      probe: httpProbe('wss://rooms.test/ws?code=ABCD2345', fetchFn),
      rand: () => 0,
    });
    for (let t = 0; t < 2 * 60_000; t += 100) {
      for (const s of sockets) if (s.readyState === FakeSocket.CONNECTING) s.refuse();
      await vi.advanceTimersByTimeAsync(100);
    }
    expect(fetchFn).toHaveBeenCalledWith('https://rooms.test/ws?code=ABCD2345', expect.anything());
    const gaps = created.slice(1).map((t, i) => t - created[i]!);
    expect(gaps.length).toBeGreaterThan(4);
    gaps.forEach((gap, i) => expect(gap).toBeGreaterThanOrEqual(nextDelay(i, () => 0)));
    expect(gaps[0]).toBeLessThan(nextDelay(REFUSED_MIN_ATTEMPT, () => 0)); // no refusal floor
    expect(states.every(([s, why]) => s === 'fatal' || why === undefined)).toBe(true);
    expect(states.some(([s]) => s === 'fatal')).toBe(false);
  });

  it('plain refusals without a probe back off on the same curve', async () => {
    const { sockets, created } = setup({ rand: () => 0.5 });
    for (let t = 0; t < 3 * 60_000; t += 100) {
      for (const s of sockets) if (s.readyState === FakeSocket.CONNECTING) s.refuse();
      await vi.advanceTimersByTimeAsync(100);
    }
    const gaps = created.slice(1).map((t, i) => t - created[i]!);
    gaps.forEach((gap, i) => expect(gap).toBeGreaterThanOrEqual(nextDelay(i, () => 0.5)));
  });

  it('given the probe reads 503, the service is paused: fatal, no retry; retry() tries once more', async () => {
    const { sockets, states, rs } = setup({ probe: async () => 503 });
    sockets[0]!.refuse();
    await vi.advanceTimersByTimeAsync(0);
    expect(state(states)).toEqual(['fatal', 'paused']);
    await vi.advanceTimersByTimeAsync(60 * 60_000);
    expect(sockets).toHaveLength(1);
    rs.retry();
    expect(sockets).toHaveLength(2);
    expect(state(states)).toEqual(['connecting', undefined]);
  });

  it('an attempt with no welcome in WELCOME_TIMEOUT_MS is dropped and retried on the backoff', () => {
    const { sockets, states, last } = setup();
    // Stuck CONNECTING (no open, no close event).
    vi.advanceTimersByTime(WELCOME_TIMEOUT_MS - 1);
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sockets[0]!.readyState).toBe(FakeSocket.CLOSED);
    expect(state(states)).toEqual(['reconnecting', undefined]);
    vi.advanceTimersByTime(nextDelay(0, () => 0));
    expect(sockets).toHaveLength(2);
    // Opened but the welcome never came: same.
    last().open();
    vi.advanceTimersByTime(WELCOME_TIMEOUT_MS);
    expect(sockets[1]!.readyState).toBe(FakeSocket.CLOSED);
    vi.advanceTimersByTime(nextDelay(1, () => 0));
    expect(sockets).toHaveLength(3);
    // A welcome in time disarms it.
    last().open();
    last().receive(welcome(V));
    vi.advanceTimersByTime(WELCOME_TIMEOUT_MS * 3);
    expect(sockets).toHaveLength(3);
    expect(state(states)).toEqual(['open', undefined]);
  });

  it('a fatal stop removes the visibility listeners; retry() adds them back', async () => {
    const { vis, sockets, rs } = setup({ probe: async () => 503 });
    const removed = vi.spyOn(vis, 'removeEventListener');
    const added = vi.spyOn(vis, 'addEventListener');
    sockets[0]!.refuse();
    await vi.advanceTimersByTimeAsync(0);
    expect(removed.mock.calls.map((c) => c[0]).sort()).toEqual(['pageshow', 'visibilitychange']);
    rs.retry();
    expect(added.mock.calls.map((c) => c[0]).sort()).toEqual(['pageshow', 'visibilitychange']);
    sockets[1]!.readyState = FakeSocket.CLOSED;
    vis.show(); // listening again: the dead attempt is noticed
    expect(sockets.length).toBeGreaterThanOrEqual(2);
  });

  it('error frames without a string message are dropped, not thrown', () => {
    const { join, last, msgs, states } = setup();
    join();
    for (const message of [undefined, null, 42, { text: 'x' }]) {
      expect(() => last().receive({ type: 'error', message } as Record<string, unknown>)).not.toThrow();
    }
    expect(msgs.map((m) => m.type)).toEqual(['welcome']);
    expect(state(states)).toEqual(['open', undefined]);
  });

  it("the old socket's 'replaced' after a resume over it is ignored (no reconnect ping-pong)", () => {
    // Phone wake: the old socket is dead to us but the Worker still holds it.
    // Our resume token replaces it there, and the Worker then tells the OLD
    // socket "opened in another window" + close 1008 "replaced".
    const { join, sockets, last, states, vis } = setup();
    join();
    const old = sockets[0]!;
    vis.hide();
    old.readyState = FakeSocket.CLOSED; // dead; close event not delivered
    vis.show();
    expect(sockets).toHaveLength(2);
    last().open();
    expect(last().ofType('join_room')[0]!.resume_token).toBe('token-1');
    last().receive(welcome(V, {}, 'token-2'));
    // Late frames on the detached socket.
    old.onmessage?.({ data: JSON.stringify({ type: 'error', message: REPLACED_WORKER }) });
    old.onclose?.({ code: 1008, reason: 'replaced', wasClean: true });
    expect(state(states)).toEqual(['open', undefined]);
    vi.advanceTimersByTime(60_000);
    expect(sockets).toHaveLength(2);
  });

  it('close() stops everything', () => {
    const { rs, join, last, sockets, vis } = setup();
    join();
    rs.close();
    expect(last().readyState).toBe(FakeSocket.CLOSED);
    vis.show();
    vi.advanceTimersByTime(60 * 60_000);
    expect(sockets).toHaveLength(1);
  });
});

describe('RoomSocket sending', () => {
  const move = (x: number): ClientMessage => ({ type: 'move_piece', piece_id: 3, x, y: 0 });

  it('drops (does not queue) messages before welcome and while reconnecting', () => {
    const { rs, last, join } = setup();
    expect(rs.send({ type: 'chat', text: 'early' })).toBe(false);
    last().open();
    expect(rs.send(move(1))).toBe(false);
    expect(rs.send({ type: 'cursor', x: 1, y: 1 })).toBe(false);
    last().receive(welcome(V));
    expect(last().json().map((m) => m.type)).toEqual(['join_room']);
    expect(rs.send({ type: 'chat', text: 'now' })).toBe(true);
    expect(last().ofType('chat')).toEqual([{ type: 'chat', text: 'now' }]);
    last().serverClose(1006);
    expect(rs.send({ type: 'chat', text: 'lost' })).toBe(false);
    void join;
  });

  it('drag_updates_throttled_to_30_per_second', () => {
    const { rs, join, last } = setup();
    join();
    for (let t = 0; t < 2000; t += 5) {
      rs.send(move(t));
      vi.advanceTimersByTime(5);
    }
    vi.advanceTimersByTime(200);
    const sent = last().framesOfType('move_piece');
    for (const f of sent) {
      const window = sent.filter((g) => g.at >= f.at && g.at < f.at + 1000).length;
      expect(window).toBeLessThanOrEqual(30);
    }
    expect(sent.length).toBeGreaterThanOrEqual(55); // ~30/s over 2 s, not starved
    for (let i = 1; i < sent.length; i++) {
      expect(sent[i]!.at - sent[i - 1]!.at).toBeGreaterThanOrEqual(DRAG_INTERVAL_MS);
    }
    // Coalesced to the latest position, with a trailing update.
    expect(JSON.parse(sent[sent.length - 1]!.data).x).toBe(1995);
  });

  it('drag_end supersedes a pending move and other messages flush it first', () => {
    const { rs, join, last } = setup();
    join();
    rs.send(move(1));
    rs.send(move(2)); // pending
    rs.send({ type: 'drag_end', piece_id: 3, x: 9, y: 9 });
    vi.advanceTimersByTime(500);
    expect(last().json().slice(1).map((m) => [m.type, m.x])).toEqual([
      ['move_piece', 1],
      ['drag_end', 9],
    ]);
    vi.advanceTimersByTime(DRAG_INTERVAL_MS);
    rs.send(move(3));
    rs.send(move(4)); // pending
    rs.send({ type: 'chat', text: 'x' });
    expect(last().json().slice(-3).map((m) => [m.type, m.x])).toEqual([
      ['move_piece', 3],
      ['move_piece', 4],
      ['chat', undefined],
    ]);
  });

  it('a pending move is dropped on disconnect and never sent on the new socket', () => {
    const { rs, join, last, sockets } = setup();
    join();
    rs.send(move(1));
    rs.send(move(2)); // pending
    last().serverClose(1006);
    vi.advanceTimersByTime(nextDelay(0, () => 0));
    expect(sockets).toHaveLength(2);
    last().open();
    last().receive(welcome(V));
    vi.advanceTimersByTime(1000);
    expect(sockets[0]!.ofType('move_piece').map((m) => m.x)).toEqual([1]);
    expect(last().ofType('move_piece')).toEqual([]);
  });

  it('cursor_updates_throttled_to_15_per_second', () => {
    const { rs, join, last } = setup();
    join();
    for (let t = 0; t < 2000; t += 4) {
      rs.send({ type: 'cursor', x: t, y: 1 });
      vi.advanceTimersByTime(4);
    }
    vi.advanceTimersByTime(200);
    const sent = last().framesOfType('cursor');
    for (const f of sent) {
      expect(sent.filter((g) => g.at >= f.at && g.at < f.at + 1000).length).toBeLessThanOrEqual(15);
    }
    expect(sent.length).toBeGreaterThanOrEqual(28);
    for (let i = 1; i < sent.length; i++) {
      expect(sent[i]!.at - sent[i - 1]!.at).toBeGreaterThanOrEqual(CURSOR_INTERVAL_MS);
    }
    expect(JSON.parse(sent[sent.length - 1]!.data).x).toBe(1996);
  });

  it('cursors are sent only when moved; cursor_hidden once on leave and cancels a pending cursor', () => {
    const { rs, join, last } = setup();
    join();
    rs.send({ type: 'cursor', x: 5, y: 5 });
    vi.advanceTimersByTime(500);
    expect(rs.send({ type: 'cursor', x: 5, y: 5 })).toBe(false);
    vi.advanceTimersByTime(500);
    expect(last().ofType('cursor')).toHaveLength(1);
    rs.send({ type: 'cursor', x: 6, y: 5 });
    rs.send({ type: 'cursor', x: 7, y: 5 }); // pending
    rs.send({ type: 'cursor_hidden' });
    expect(rs.send({ type: 'cursor_hidden' })).toBe(false);
    vi.advanceTimersByTime(500);
    expect(last().json().slice(1).map((m) => [m.type, m.x])).toEqual([
      ['cursor', 5],
      ['cursor', 6],
      ['cursor_hidden', undefined],
    ]);
    // Visible again at the same spot: sent.
    expect(rs.send({ type: 'cursor', x: 6, y: 5 })).toBe(true);
  });

  it('chat and board pings are not throttled client-side', () => {
    const { rs, join, last } = setup();
    join();
    for (let i = 0; i < 6; i++) rs.send({ type: 'chat', text: `m${i}` });
    for (let i = 0; i < 4; i++) rs.send({ type: 'ping', x: i, y: 0 });
    expect(last().ofType('chat')).toHaveLength(6);
    expect(last().ofType('ping')).toHaveLength(4);
  });
});
