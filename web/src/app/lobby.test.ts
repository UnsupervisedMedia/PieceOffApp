// @vitest-environment node
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { initWasm } from '../test-wasm';
import { FakeVisibility } from '../net/testkit';
import { LOBBY_POLL_MS, LobbyError, LobbyPoller, fetchLobby, quickMatch, type LobbyRoom } from './lobby';

beforeAll(async () => {
  await initWasm();
});
beforeEach(() => {
  vi.useFakeTimers({ now: 1_000_000 });
});
afterEach(() => {
  vi.useRealTimers();
});

const API = 'https://rooms.example';

function entry(code: string, players: number, over: Partial<LobbyRoom> = {}): Record<string, unknown> {
  return {
    code,
    title: 'Tidal Glass',
    host_name: 'Ann',
    players,
    max_players: 8,
    spectators: 0,
    pieces: 48,
    placed: 12,
    cut_style: 'jigsaw',
    created_at: 100,
    updated_at: 200,
    ...over,
  };
}

const ok = (rooms: unknown[]) =>
  new Response(JSON.stringify({ rooms }), { status: 200, headers: { 'content-type': 'application/json' } });

describe('lobby', () => {
  it('fetch_lobby_omits_credentials_and_cleans_entries', async () => {
    const fetchFn = vi.fn(async (_url: string, _init?: RequestInit) =>
      ok([
        entry('abcd2345', 2, { title: 'Tidal\u0000 Glass', host_name: 'pornstar' }),
        entry('bad', 1),
        { code: 'WXYZ6789', players: 'x' },
        entry('WXYZ6789', 8),
        entry('QRST2345', 1, { max_players: 0 }),
        entry('MNPQ2345', 3, { placed: 999 }),
      ]),
    );
    const rooms = await fetchLobby(API, { fetch: fetchFn as unknown as typeof fetch });
    expect(fetchFn).toHaveBeenCalledTimes(1);
    const [url, init] = fetchFn.mock.calls[0]!;
    expect(url).toBe(`${API}/api/lobby`);
    expect(init).toMatchObject({ credentials: 'omit', cache: 'no-store' });
    expect(rooms.map((r) => r.code)).toEqual(['MNPQ2345', 'ABCD2345', 'WXYZ6789']);
    const abcd = rooms.find((r) => r.code === 'ABCD2345')!;
    expect(abcd.title).toBe('Tidal Glass');
    expect(abcd.host_name).toBe(''); // the public filter blanks it, like desktop
    expect(rooms.find((r) => r.code === 'MNPQ2345')!.placed).toBe(48);
  });

  it('fetch_lobby_maps_failures', async () => {
    const f = (r: Response | Error) =>
      (async () => {
        if (r instanceof Error) throw r;
        return r;
      }) as unknown as typeof fetch;
    await expect(fetchLobby(API, { fetch: f(new Response('{}', { status: 429 })) })).rejects.toMatchObject({
      kind: 'rate-limited',
    });
    await expect(fetchLobby(API, { fetch: f(new Response('{}', { status: 503 })) })).rejects.toMatchObject({
      kind: 'paused',
    });
    await expect(fetchLobby(API, { fetch: f(new TypeError('offline')) })).rejects.toBeInstanceOf(LobbyError);
    await expect(fetchLobby(API, { fetch: f(new Response('nope', { status: 200 })) })).rejects.toMatchObject({
      kind: 'bad-response',
    });
  });

  it('lobby_poll_every_10s_and_stops_when_hidden', async () => {
    expect(LOBBY_POLL_MS).toBe(10_000);
    const vis = new FakeVisibility();
    const fetchFn = vi.fn(async () => ok([entry('ABCD2345', 1)]));
    const seen: number[] = [];
    const poller = new LobbyPoller({ api: API, fetch: fetchFn as unknown as typeof fetch, visibility: vis });
    poller.onUpdate((u) => {
      if (u.rooms) seen.push(u.rooms.length);
    });
    poller.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchFn).toHaveBeenCalledTimes(1);
    expect(seen).toEqual([1]);
    await vi.advanceTimersByTimeAsync(9_999);
    expect(fetchFn).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetchFn).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fetchFn).toHaveBeenCalledTimes(3);

    vis.hide();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetchFn).toHaveBeenCalledTimes(3);

    vis.show(); // visibilitychange + pageshow: one immediate refresh, not two
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchFn).toHaveBeenCalledTimes(4);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fetchFn).toHaveBeenCalledTimes(5);

    poller.stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetchFn).toHaveBeenCalledTimes(5);
    vis.show();
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchFn).toHaveBeenCalledTimes(5);
  });

  it('lobby_poll_waits_longer_after_429', async () => {
    const vis = new FakeVisibility();
    const fetchFn = vi.fn(async () => new Response('{}', { status: 429 }));
    const errors: string[] = [];
    const poller = new LobbyPoller({ api: API, fetch: fetchFn as unknown as typeof fetch, visibility: vis });
    poller.onUpdate((u) => {
      if (u.error) errors.push(u.error.kind);
    });
    poller.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(errors).toEqual(['rate-limited']);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fetchFn).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(50_000);
    expect(fetchFn).toHaveBeenCalledTimes(2);
    poller.stop();
  });

  it('lobby_poll_starts_only_when_visible', async () => {
    const vis = new FakeVisibility();
    vis.visibilityState = 'hidden';
    const fetchFn = vi.fn(async () => ok([]));
    const poller = new LobbyPoller({ api: API, fetch: fetchFn as unknown as typeof fetch, visibility: vis });
    poller.start();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(fetchFn).not.toHaveBeenCalled();
    vis.show();
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchFn).toHaveBeenCalledTimes(1);
    poller.stop();
  });

  it('quick_match_prefers_rooms_with_space', () => {
    const r = (code: string, players: number, max = 8, created = 100, placed = 0): LobbyRoom => ({
      code,
      title: 'T',
      host_name: '',
      players,
      max_players: max,
      spectators: 0,
      pieces: 48,
      placed,
      cut_style: 'jigsaw',
      created_at: created,
    });
    // A full room loses to any room with a seat, however far along it is.
    expect(quickMatch([r('AAAAAAAA', 8, 8, 100, 47), r('BBBBBBBB', 2), r('CCCCCCCC', 3)])!.code).toBe('CCCCCCCC');
    // Busier first, then newer, then code: deterministic whatever the input order.
    const tie = [r('DDDDDDDD', 3, 8, 100), r('EEEEEEEE', 3, 8, 300), r('FFFFFFFF', 3, 8, 300)];
    expect(quickMatch(tie)!.code).toBe('EEEEEEEE');
    expect(quickMatch([...tie].reverse())!.code).toBe('EEEEEEEE');
    expect(quickMatch([r('AAAAAAAA', 8)])).toBeNull();
    expect(quickMatch([])).toBeNull();
  });
});
