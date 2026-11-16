// @vitest-environment node
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { protocol_version } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import { FakeSocket, FakeVisibility, welcome } from '../net/testkit';
import { ROOM_FULL, codeFromSearch, joinFlow, parseCode, type JoinState } from './join';
import { memoryStorage, saveResume, type AppStorage } from './names';

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

const CODE = 'ABCD2345';
const NAME_TAKEN =
  'That name is taken in this room. Pick another name, or ask the host to release it (names free up 10 minutes after their player leaves).';
const PUBLIC_NAME_REFUSED = 'Pick a different name for public rooms';
const REMOVED_BY_HOST = 'The host removed you from this room.';

function setup(o: { storage?: AppStorage; probe?: () => Promise<number | null> } = {}) {
  const sockets: FakeSocket[] = [];
  const storage = o.storage ?? { local: memoryStorage(), session: memoryStorage() };
  const flow = joinFlow({
    api: 'https://rooms.example',
    storage,
    socket: {
      makeSocket: (url) => {
        const s = new FakeSocket(url);
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      probe: o.probe ?? null,
      visibility: new FakeVisibility(),
      rand: () => 0,
      keepalive: false,
    },
  });
  const states: JoinState[] = [];
  flow.state.subscribe((s) => states.push(s));
  const joins = () => sockets.flatMap((s) => s.ofType('join_room'));
  return { flow, sockets, storage, states, joins };
}

describe('join flow', () => {
  it('room_full_joins_as_spectator', () => {
    const { flow, sockets, joins } = setup();
    flow.start(CODE);
    expect(flow.state.get().kind).toBe('asking-name');
    flow.submitName('Ann');
    expect(flow.state.get().kind).toBe('checking');
    expect(sockets).toHaveLength(1);
    expect(sockets[0]!.url).toContain('wss://rooms.example/ws?code=ABCD2345');
    sockets[0]!.open();
    expect(joins()[0]).toMatchObject({ invite_code: CODE, name: 'Ann', spectator: false });

    // Core refuses a 9th player, then the room closes the socket.
    sockets[0]!.receive({ type: 'error', message: ROOM_FULL });
    sockets[0]!.serverClose(1008, 'join refused');
    expect(sockets).toHaveLength(2);
    sockets[1]!.open();
    expect(joins()[1]).toMatchObject({ invite_code: CODE, name: 'Ann', spectator: true });
    sockets[1]!.receive(welcome(V, { spectator: true }));
    const s = flow.state.get();
    expect(s.kind).toBe('room-full-spectating');

    // The refused player socket never retries.
    vi.advanceTimersByTime(120_000);
    expect(sockets).toHaveLength(2);
    expect(joins()).toHaveLength(2);
  });

  it('spectate_on_request_joins_as_spectator', () => {
    const { flow, sockets, joins } = setup();
    flow.start(CODE, { spectator: true });
    flow.submitName('Ann');
    sockets[0]!.open();
    expect(joins()[0]).toMatchObject({ spectator: true });
    sockets[0]!.receive(welcome(V, { spectator: true }));
    expect(flow.state.get().kind).toBe('joined-spectator');
  });

  it('name_taken_reprompts_and_never_sends_second_join_automatically', () => {
    const { flow, sockets, joins } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive({ type: 'error', message: NAME_TAKEN });
    sockets[0]!.serverClose(1008, 'join refused');
    const s = flow.state.get();
    expect(s).toMatchObject({ kind: 'name-taken', reason: 'taken', message: NAME_TAKEN, name: 'Ann' });

    vi.advanceTimersByTime(10 * 60_000);
    expect(sockets).toHaveLength(1);
    expect(joins()).toHaveLength(1);

    // Only the player's own new name sends another join.
    flow.submitName('Bob');
    expect(sockets).toHaveLength(2);
    sockets[1]!.open();
    expect(joins()[1]).toMatchObject({ name: 'Bob' });
    sockets[1]!.receive(welcome(V, { name: 'Bob' }));
    expect(flow.state.get().kind).toBe('joined-player');
  });

  it('public_name_refusal_from_server_reprompts', () => {
    const { flow, sockets, joins } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive({ type: 'error', message: PUBLIC_NAME_REFUSED });
    expect(flow.state.get()).toMatchObject({ kind: 'name-taken', reason: 'not-allowed' });
    vi.advanceTimersByTime(120_000);
    expect(joins()).toHaveLength(1);
  });

  it('blocklisted_name_rejected_before_network_in_public_room', () => {
    const { flow, sockets } = setup();
    flow.start(CODE, { listed: true });
    flow.submitName('pornstar');
    expect(flow.state.get()).toMatchObject({ kind: 'name-taken', reason: 'not-allowed', message: PUBLIC_NAME_REFUSED });
    expect(sockets).toHaveLength(0);
    // An acceptable name goes through.
    flow.submitName('Ada');
    expect(sockets).toHaveLength(1);
  });

  it('blocklist_is_not_applied_client_side_in_invite_rooms', () => {
    const { flow, sockets } = setup();
    flow.start(CODE);
    flow.submitName('pornstar');
    expect(sockets).toHaveLength(1);
  });

  it('names_are_cleaned_like_core_before_sending', () => {
    const { flow, sockets, joins } = setup();
    flow.start(CODE);
    flow.submitName('  \u0007Zed' + 'z'.repeat(40) + '  ');
    sockets[0]!.open();
    const name = joins()[0]!.name as string;
    expect(name.startsWith('Zed')).toBe(true);
    // Core keeps 24 characters (two of them the leading spaces), then trims.
    expect([...name]).toHaveLength(22);
  });

  it('invalid_lowercase_and_spaced_codes_normalise', () => {
    expect(parseCode(' abcd-2345 ')).toBe(CODE);
    expect(parseCode('abcd 2345')).toBe(CODE);
    expect(parseCode('ABCD1234')).toBeNull(); // 1 is not in the alphabet
    expect(parseCode('ABCDO234')).toBeNull(); // nor O
    expect(parseCode('ABC2345')).toBeNull();
    expect(parseCode('x'.repeat(100))).toBeNull();
    expect(parseCode(null)).toBeNull();
    const { flow, sockets } = setup();
    flow.start('nope');
    expect(flow.state.get()).toMatchObject({ kind: 'room-not-found', invalid: true });
    flow.start(' abcd-2345');
    expect(flow.state.get()).toMatchObject({ kind: 'asking-name', code: CODE });
    expect(sockets).toHaveLength(0);
  });

  it('join_link_query_is_parsed', () => {
    expect(codeFromSearch('?c=abcd2345')).toBe(CODE);
    expect(codeFromSearch('c=ABCD-2345&x=1')).toBe(CODE);
    expect(codeFromSearch('?c=<script>')).toBeNull();
    expect(codeFromSearch('')).toBeNull();
  });

  it('unknown_room_is_room_not_found_without_retry', () => {
    const { flow, sockets } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive({ type: 'error', message: 'Room code not found' });
    sockets[0]!.serverClose(1008, 'Room code not found');
    expect(flow.state.get()).toMatchObject({ kind: 'room-not-found', invalid: false, code: CODE });
    vi.advanceTimersByTime(120_000);
    expect(sockets).toHaveLength(1);
  });

  it('banned_before_welcome_is_fatal_banned', () => {
    const { flow, sockets } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive({ type: 'error', message: REMOVED_BY_HOST });
    expect(flow.state.get()).toMatchObject({ kind: 'fatal', reason: 'banned' });
    vi.advanceTimersByTime(120_000);
    expect(sockets).toHaveLength(1);
  });

  it('paused_service_is_fatal_paused', async () => {
    const { flow, sockets } = setup({ probe: async () => 503 });
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.refuse();
    await vi.advanceTimersByTimeAsync(0);
    expect(flow.state.get()).toMatchObject({ kind: 'fatal', reason: 'paused' });
    vi.advanceTimersByTime(120_000);
    expect(sockets).toHaveLength(1);
  });

  it('rate_limited_join_shows_waiting_and_backs_off', async () => {
    const { flow, sockets } = setup({ probe: async () => 429 });
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.refuse();
    await vi.advanceTimersByTimeAsync(0);
    expect(flow.state.get()).toMatchObject({ kind: 'checking', rateLimited: true });
    // The socket's refusal floor (8 s) holds: no immediate retry.
    vi.advanceTimersByTime(7_000);
    expect(sockets).toHaveLength(1);
  });

  it('kicked_after_welcome_is_fatal_and_forgets_the_room', () => {
    const { flow, sockets, storage } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive(welcome(V));
    expect(storage.session!.getItem('pieceoff.web.resume.' + CODE)).not.toBeNull();
    sockets[0]!.receive({ type: 'kicked', client_id: 'me', message: 'bye' });
    expect(flow.state.get()).toMatchObject({ kind: 'fatal', reason: 'kicked' });
    expect(storage.session!.getItem('pieceoff.web.resume.' + CODE)).toBeNull();
  });

  it('protocol_mismatch_is_fatal_update', () => {
    const { flow, sockets } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive(welcome(V + 1));
    expect(flow.state.get()).toMatchObject({ kind: 'fatal', reason: 'update-client' });
  });

  it('player_key_is_sent_and_stable', () => {
    const storage = { local: memoryStorage(), session: memoryStorage() };
    const a = setup({ storage });
    a.flow.start(CODE);
    a.flow.submitName('Ann');
    a.sockets[0]!.open();
    const key = a.joins()[0]!.player_key as string;
    expect(key).toMatch(/^[0-9a-f]{32}$/);
    const b = setup({ storage });
    b.flow.start('WXYZ6789');
    b.flow.submitName('Ann');
    b.sockets[0]!.open();
    expect(b.joins()[0]!.player_key).toBe(key);
  });

  it('reload_resumes_with_saved_token_and_name', () => {
    const storage = { local: memoryStorage(), session: memoryStorage() };
    const first = setup({ storage });
    first.flow.start(CODE);
    first.flow.submitName('Ann');
    first.sockets[0]!.open();
    first.sockets[0]!.receive(welcome(V, { name: 'Ann' }, 'tok-A'));
    expect(first.flow.state.get().kind).toBe('joined-player');

    // A page reload: a fresh flow on the same storage goes straight back in.
    const again = setup({ storage });
    again.flow.start(CODE.toLowerCase());
    expect(again.flow.state.get().kind).toBe('checking');
    again.sockets[0]!.open();
    expect(again.joins()[0]).toMatchObject({ name: 'Ann', resume_token: 'tok-A', spectator: false });
  });

  it('creator_token_from_create_resumes_as_host_after_reload', () => {
    const storage = { local: memoryStorage(), session: memoryStorage() };
    saveResume(storage, CODE, { token: 'creator-secret', name: 'Host', spectator: false });
    const { flow, sockets, joins } = setup({ storage });
    flow.start(CODE);
    sockets[0]!.open();
    expect(joins()[0]).toMatchObject({ name: 'Host', resume_token: 'creator-secret' });
    // The welcome's token replaces the stored one.
    sockets[0]!.receive(welcome(V, { name: 'Host' }, 'next-secret'));
    const saved = JSON.parse(storage.session.getItem('pieceoff.web.resume.' + CODE)!);
    expect(saved.token).toBe('next-secret');
  });

  it('creator_token_held_in_memory_without_session_storage', () => {
    const storage = { local: null, session: null };
    const { flow, sockets, joins } = setup({ storage });
    flow.start(CODE, { listed: true, resume: { token: 'creator-secret', name: 'Host', spectator: false } });
    expect(flow.state.get().kind).toBe('checking');
    sockets[0]!.open();
    expect(joins()[0]).toMatchObject({ name: 'Host', resume_token: 'creator-secret' });

    // A name refusal re-prompts; the next join keeps the creator's identity.
    sockets[0]!.receive({ type: 'error', message: NAME_TAKEN });
    expect(flow.state.get().kind).toBe('name-taken');
    flow.submitName('Host Two');
    sockets[1]!.open();
    expect(joins()[1]).toMatchObject({ name: 'Host Two', resume_token: 'creator-secret' });

    // The welcome's token replaces it in memory, and a later start in this page uses it.
    sockets[1]!.receive(welcome(V, { name: 'Host Two' }, 'next-secret'));
    sockets[1]!.receive({ type: 'error', message: 'This name was opened in another window or on another device using the same settings' });
    expect(flow.state.get()).toMatchObject({ kind: 'fatal', reason: 'opened-elsewhere' });
    flow.start(CODE);
    sockets[2]!.open();
    expect(joins()[2]).toMatchObject({ name: 'Host Two', resume_token: 'next-secret' });

    // Leaving forgets it.
    flow.leave();
    flow.start(CODE);
    expect(flow.state.get().kind).toBe('asking-name');
  });

  it('submit_name_is_ignored_while_checking', () => {
    const { flow, sockets } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    flow.submitName('Ann');
    expect(sockets).toHaveLength(1);
  });

  it('leave_closes_the_socket_and_forgets_the_room', () => {
    const { flow, sockets, storage } = setup();
    flow.start(CODE);
    flow.submitName('Ann');
    sockets[0]!.open();
    sockets[0]!.receive(welcome(V));
    flow.leave();
    expect(sockets[0]!.closedWith).not.toBeNull();
    expect(flow.state.get().kind).toBe('idle');
    expect(storage.session!.getItem('pieceoff.web.resume.' + CODE)).toBeNull();
  });
});
