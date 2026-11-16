import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { protocol_version } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import { FakeSocket, FakeVisibility, welcome } from '../net/testkit';
import { DEFAULT_CONFIG } from './config';
import { BUILTIN_MEDIA } from './create';
import { startApp, type App } from './main';
import { memoryStorage, type AppStorage } from './names';
import type { TurnstileApi } from './turnstile';

let V: number;
beforeAll(async () => {
  await initWasm();
  V = protocol_version();
});

let app: App | null = null;
afterEach(() => {
  app?.destroy();
  app = null;
  document.body.replaceChildren();
});

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

const lobbyRoom = (code: string, players: number) => ({
  code,
  title: 'Tidal Glass',
  host_name: 'Ann',
  players,
  max_players: 8,
  spectators: 0,
  pieces: 48,
  placed: 0,
  cut_style: 'jigsaw',
  created_at: 1,
});

function setup(o: { search?: string; rooms?: unknown[]; create?: () => Response; storage?: AppStorage } = {}) {
  const root = document.createElement('div');
  document.body.append(root);
  const sockets: FakeSocket[] = [];
  const urls: string[] = [];
  const storage = o.storage ?? { local: memoryStorage(), session: memoryStorage() };
  const posts: RequestInit[] = [];
  const fetchFn = vi.fn(async (url: string, init?: RequestInit) => {
    if (url.endsWith('/api/lobby')) return json(200, { rooms: o.rooms ?? [] });
    if (url.endsWith('/api/rooms')) {
      posts.push(init!);
      return o.create ? o.create() : json(200, { code: 'NEWR2345', client_id: 'host-id', resume_token: 'creator-secret' });
    }
    return json(404, {});
  });
  let tsCallback: ((t: string) => void) | null = null;
  const ts: TurnstileApi = {
    render: vi.fn((_el: HTMLElement, opts: Record<string, unknown>) => {
      tsCallback = opts.callback as (t: string) => void;
      return 'w';
    }),
    reset: vi.fn(),
    remove: vi.fn(),
  };
  const reload = vi.fn();
  const loc = { search: o.search ?? '', pathname: '/play/' };
  app = startApp({
    root,
    config: { ...DEFAULT_CONFIG, api: 'https://rooms.example' },
    storage,
    fetch: fetchFn as unknown as typeof fetch,
    location: loc,
    history: {
      replaceState: (_d, _u, url) => {
        urls.push(String(url));
        const q = String(url).split('?')[1];
        loc.search = q ? `?${q}` : '';
      },
    },
    reload,
    turnstile: async () => ts,
    visibility: new FakeVisibility(),
    random: () => 0,
    socket: {
      makeSocket: (url) => {
        const s = new FakeSocket(url);
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      probe: null,
      visibility: new FakeVisibility(),
      keepalive: false,
    },
    loadBoard: async () => ({
      mountBoard: (host, joined) => {
        host.textContent = `board ${joined.code} ${joined.name}`;
        return { destroy: () => undefined };
      },
    }),
  });
  const token = () => tsCallback?.('turnstile-ok');
  return { root, sockets, urls, storage, fetchFn, posts, ts, token, reload, loc };
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const click = (root: HTMLElement, text: string) => {
  const b = [...root.querySelectorAll('button')].find((x) => x.textContent === text);
  if (!b) throw new Error(`no button ${text}: ${root.textContent}`);
  b.click();
};
const submit = (root: HTMLElement) => root.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));

describe('app', () => {
  it('join_link_opens_the_name_prompt_for_that_room', async () => {
    const { root, sockets, loc } = setup({ search: '?c=abcd2345' });
    expect(root.querySelector('h1')!.textContent).toBe('Join room ABCD2345');
    expect(loc.search).toBe('?c=ABCD2345');
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Ann';
    submit(root);
    expect(sockets).toHaveLength(1);
    sockets[0]!.open();
    sockets[0]!.receive(welcome(V, { name: 'Ann' }));
    await flush();
    expect(root.textContent).toContain('board ABCD2345 Ann');
  });

  it('enter_code_validates_before_joining', () => {
    const { root, sockets } = setup();
    const input = root.querySelector<HTMLInputElement>('input[name=code]')!;
    input.value = 'nope';
    submit(root);
    expect(root.textContent).toContain('Room codes are 8 letters and digits');
    expect(sockets).toHaveLength(0);
    input.value = 'abcd-2345';
    submit(root);
    expect(root.querySelector('h1')!.textContent).toBe('Join room ABCD2345');
  });

  it('quick_match_joins_the_best_open_room', async () => {
    const { root } = setup({ rooms: [lobbyRoom('FULL2345', 8), lobbyRoom('PENN2345', 3)] });
    await flush();
    expect(root.querySelectorAll('li.room')).toHaveLength(2);
    click(root, 'Quick match');
    expect(root.querySelector('h1')!.textContent).toBe('Join room PENN2345');
    // A public room: the client-side name filter applies.
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'pornstar';
    submit(root);
    expect(root.querySelector('[role=alert]')!.textContent).toBe('Pick a different name for public rooms');
  });

  it('quick_match_with_no_seat_offers_a_public_room', async () => {
    const { root, ts } = setup({ rooms: [lobbyRoom('FULL2345', 8)] });
    await flush();
    click(root, 'Quick match');
    await flush();
    expect(root.querySelector('h1')!.textContent).toBe('Create a room');
    expect(root.querySelector<HTMLInputElement>('input[name=listed]')!.checked).toBe(true);
    expect(root.querySelector<HTMLInputElement>('input[name=puzzle]:checked')!.value).toBe(BUILTIN_MEDIA[0]!.sha256);
    expect(ts.render).toHaveBeenCalledTimes(1);
  });

  it('create_then_reload_resumes_as_host', async () => {
    const storage = { local: memoryStorage(), session: memoryStorage() };
    const a = setup({ storage });
    click(a.root, 'Create room');
    await flush();
    a.root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Hostie';
    a.token();
    submit(a.root);
    await flush();
    expect(a.posts).toHaveLength(1);
    expect(new Headers(a.posts[0]!.headers).get('x-turnstile-token')).toBe('turnstile-ok');
    expect(a.loc.search).toBe('?c=NEWR2345');
    expect(a.sockets).toHaveLength(1);
    a.sockets[0]!.open();
    expect(a.sockets[0]!.ofType('join_room')[0]).toMatchObject({ name: 'Hostie', resume_token: 'creator-secret' });
    a.sockets[0]!.receive(welcome(V, { name: 'Hostie' }, 'welcome-secret'));
    await flush();
    app!.destroy();
    app = null;

    // Same tab after a reload: same storage and URL, straight back in as host.
    // (destroy() above left the room, which forgets it; put the state back as a
    // real reload would have kept it.)
    storage.session.setItem('pieceoff.web.resume.NEWR2345', JSON.stringify({ token: 'welcome-secret', name: 'Hostie', spectator: false }));
    const b = setup({ storage, search: '?c=NEWR2345' });
    expect(b.sockets).toHaveLength(1);
    b.sockets[0]!.open();
    expect(b.sockets[0]!.ofType('join_room')[0]).toMatchObject({ name: 'Hostie', resume_token: 'welcome-secret' });
  });

  const throwing = () =>
    ({
      length: 0,
      key: () => null,
      clear: () => undefined,
      getItem: () => {
        throw new Error('SecurityError');
      },
      setItem: () => {
        throw new Error('QuotaExceededError');
      },
      removeItem: () => {
        throw new Error('SecurityError');
      },
    }) as unknown as Storage;

  for (const [label, storage] of [
    ['create_without_session_storage_joins_as_host', { local: null, session: null }],
    ['create_with_throwing_storage_joins_as_host', { local: throwing(), session: throwing() }],
  ] as const) {
    it(label, async () => {
      const a = setup({ storage: storage as AppStorage });
      click(a.root, 'Create room');
      await flush();
      a.root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Hostie';
      a.token();
      submit(a.root);
      await flush();
      expect(a.posts).toHaveLength(1);
      // No name prompt: straight into the room with the creator's secret.
      expect(a.root.querySelector('input[name=name]')).toBeNull();
      expect(a.sockets).toHaveLength(1);
      a.sockets[0]!.open();
      const join = a.sockets[0]!.ofType('join_room')[0]!;
      expect(join).toMatchObject({ invite_code: 'NEWR2345', name: 'Hostie', resume_token: 'creator-secret' });
      expect(join.player_key).toMatch(/^[0-9a-f]{32}$/);
      // The server adopts the creator's id, which is the room's host.
      const w = welcome(V, { client_id: 'host-id', name: 'Hostie' }, 'next-secret');
      w.snapshot.host_id = 'host-id';
      a.sockets[0]!.receive(w);
      await flush();
      const s = app!.join.state.get();
      expect(s.kind).toBe('joined-player');
      if (s.kind === 'joined-player') {
        expect(s.room.snapshot()!.host_id).toBe(s.room.clientId);
        expect(s.socket.resumeToken).toBe('next-secret');
      }
      expect(a.root.textContent).toContain('board NEWR2345 Hostie');
    });
  }

  it('create_429_shows_message_resets_check_and_cools_down', async () => {
    const { root, posts, ts, token } = setup({ create: () => json(429, { error: 'Too many rooms created. Try again later.' }) });
    click(root, 'Create room');
    await flush();
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Ann';
    token();
    submit(root);
    await flush();
    expect(posts).toHaveLength(1);
    expect(root.querySelector('.screen--create [role=alert]')!.textContent).toMatch(/Too many rooms/);
    expect(ts.reset).toHaveBeenCalledTimes(1);
    const btn = root.querySelector<HTMLButtonElement>('button[type=submit]')!;
    token(); // a fresh check does not lift the cooldown
    expect(btn.disabled).toBe(true);
    submit(root);
    await flush();
    expect(posts).toHaveLength(1);
  });

  it('room_full_banner_and_fatal_screens', async () => {
    const { root, sockets, reload } = setup({ search: '?c=ABCD2345' });
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Ann';
    submit(root);
    sockets[0]!.open();
    sockets[0]!.receive({ type: 'error', message: 'This room already has 8 players' });
    sockets[1]!.open();
    sockets[1]!.receive(welcome(V, { spectator: true }));
    await flush();
    expect(root.querySelector('.banner')!.textContent).toContain('already has 8 players');
    // The server moves on to a newer protocol: full-screen reload.
    sockets[1]!.serverClose(1006);
    expect(root.textContent).toContain('board');
    app!.join.leave();
    expect(root.querySelector('h1')!.textContent).toBe('Play PieceOff');
    app!.join.start('ABCD2345');
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Ann';
    submit(root);
    sockets.at(-1)!.open();
    sockets.at(-1)!.receive(welcome(V + 1));
    expect(root.querySelector('h1')!.textContent).toBe('PieceOff has been updated');
    click(root, 'Reload to update');
    expect(reload).toHaveBeenCalled();
  });

  it('kicked_or_banned_in_room_ends_on_a_screen_with_back_to_lobby', async () => {
    const { root, sockets } = setup({ search: '?c=ABCD2345' });
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Ann';
    submit(root);
    sockets[0]!.open();
    sockets[0]!.receive(welcome(V));
    await flush();
    expect(root.textContent).toContain('board ABCD2345 Ann');
    sockets[0]!.receive({ type: 'kicked', client_id: 'me', message: 'removed by host' });
    expect(root.querySelector('[data-end]')!.getAttribute('data-end')).toBe('kicked');
    expect(root.textContent).not.toContain('board ABCD2345');
    click(root, 'Back to the lobby');
    expect(root.querySelector('h1')!.textContent).toBe('Play PieceOff');
    // Rejoining after a ban: the refusal is fatal 'banned', same way out.
    app!.join.start('ABCD2345');
    root.querySelector<HTMLInputElement>('input[name=name]')!.value = 'Ann';
    submit(root);
    sockets.at(-1)!.open();
    sockets.at(-1)!.receive({ type: 'error', message: 'The host removed you from this room.' });
    expect(root.querySelector('[data-end]')!.getAttribute('data-end')).toBe('banned');
    click(root, 'Back to the lobby');
    expect(root.querySelector('h1')!.textContent).toBe('Play PieceOff');
  });

  it('escape_goes_back_from_the_name_prompt', () => {
    const { root } = setup({ search: '?c=ABCD2345' });
    root.querySelector('input')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(root.querySelector('h1')!.textContent).toBe('Play PieceOff');
  });
});
