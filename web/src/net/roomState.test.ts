// @vitest-environment node
import { beforeAll, describe, expect, it } from 'vitest';
import { protocol_version } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import type { ChatLine, Piece, PieceHolder, ServerMessage, SolveStats } from '../protocol';
import { CHAT_KEEP, RoomState } from './roomState';
import type { FatalReason, SocketState } from './socket';
import { pieces, snapshot, welcome } from './testkit';

let V: number;
beforeAll(async () => {
  await initWasm();
  V = protocol_version();
});

function joined(now = 50_000, over = {}) {
  let t = now;
  const rs = new RoomState({ now: () => t });
  const changes: Array<ServerMessage | null> = [];
  rs.onChange((m) => changes.push(m));
  rs.applyServer(welcome(V, over));
  return { rs, changes, setNow: (v: number) => (t = v) };
}

const held = (p: Piece, by: string | null, x = p.x, y = p.y): Piece => ({ ...p, held_by: by, x, y });

describe('RoomState welcome', () => {
  it('stores the snapshot, identity, peers and the server clock offset', () => {
    const { rs, changes } = joined(50_000, {
      server_now_ms: 80_000,
      peers: ['Bo', 'Cy'],
      spectators: ['Di'],
      client_id: 'me',
      name: 'Ann',
    });
    expect(rs.snapshot()?.invite_code).toBe('ABCD2345');
    expect(rs.clientId).toBe('me');
    expect(rs.name).toBe('Ann');
    expect(rs.spectator).toBe(false);
    expect(rs.peers).toEqual(['Bo', 'Cy']);
    expect(rs.spectators).toEqual(['Di']);
    expect(rs.serverClockOffsetMs).toBe(30_000);
    expect(rs.serverNow()).toBe(80_000);
    expect(changes.map((m) => m?.type)).toEqual(['welcome']);
  });

  it('an unknown server clock (0) leaves the offset at 0', () => {
    const { rs } = joined(50_000, { server_now_ms: 0 });
    expect(rs.serverClockOffsetMs).toBe(0);
    expect(rs.serverNow()).toBe(50_000);
  });

  it('nothing but a welcome or snapshot creates a board', () => {
    const rs = new RoomState();
    rs.applyServer({ type: 'pieces_changed', pieces: pieces(1), revision: 9 });
    expect(rs.snapshot()).toBeNull();
  });
});

describe('RoomState pieces_changed', () => {
  it('replaces only the listed pieces and bumps the revision', () => {
    const { rs } = joined();
    const before = rs.snapshot()!.pieces.map((p) => ({ ...p }));
    const moved = { ...before[2]!, x: 5, y: 6, group_id: 0 };
    rs.applyServer({ type: 'pieces_changed', pieces: [moved], revision: 4 });
    const snap = rs.snapshot()!;
    expect(snap.revision).toBe(4);
    expect(snap.pieces[2]).toEqual(moved);
    expect(snap.pieces[0]).toEqual(before[0]);
    expect(snap.pieces[3]).toEqual(before[3]);
  });

  it('updates stats and holders only when sent', () => {
    const { rs } = joined();
    const stats: SolveStats = {
      elapsed_ms: 10,
      running_since_ms: 99,
      started: true,
      finished: false,
      players: [{ name: 'Ann', pieces: 2 }],
    };
    const holders: PieceHolder[] = [
      { id: 1, owner: 'Ann', label: 'edges', x: 0, y: 0, width: 100, height: 100, owner_away_since_ms: null },
    ];
    rs.applyServer({ type: 'pieces_changed', pieces: [], revision: 2, stats, holders });
    expect(rs.snapshot()!.stats).toEqual(stats);
    expect(rs.snapshot()!.holders).toEqual(holders);
    rs.applyServer({ type: 'pieces_changed', pieces: [], revision: 3, stats: null });
    rs.applyServer({ type: 'pieces_changed', pieces: [], revision: 4 });
    expect(rs.snapshot()!.stats).toEqual(stats);
    expect(rs.snapshot()!.holders).toEqual(holders);
    rs.applyServer({ type: 'pieces_changed', pieces: [], revision: 5, holders: [] });
    expect(rs.snapshot()!.holders).toEqual([]);
  });

  it('ignores stale revisions, like the desktop client', () => {
    const { rs } = joined();
    rs.applyServer({ type: 'pieces_changed', pieces: [], revision: 7 });
    const p = rs.snapshot()!.pieces[0]!;
    rs.applyServer({ type: 'pieces_changed', pieces: [{ ...p, x: -1 }], revision: 6 });
    expect(rs.snapshot()!.pieces[0]!.x).toBe(p.x);
    expect(rs.snapshot()!.revision).toBe(7);
    rs.applyServer({ type: 'playback_changed', playback: { ...rs.snapshot()!.playback, playing: false }, revision: 5 });
    expect(rs.snapshot()!.playback.playing).toBe(true);
    rs.applyServer({ type: 'playback_changed', playback: { ...rs.snapshot()!.playback, playing: false }, revision: 7 });
    expect(rs.snapshot()!.playback.playing).toBe(false);
  });

  it('ignores pieces it does not know', () => {
    const { rs } = joined();
    rs.applyServer({ type: 'pieces_changed', pieces: [{ ...pieces(1)[0]!, id: 999 }], revision: 2 });
    expect(rs.snapshot()!.pieces).toHaveLength(4);
  });
});

describe('RoomState local drag overlay', () => {
  it('snapshot_replaces_state_and_clears_optimistic_drag', () => {
    const { rs, changes } = joined();
    rs.beginDrag(1);
    rs.dragTo(42, 43);
    expect(rs.localDrag()).not.toBeNull();
    expect(rs.pieces()[1]).toMatchObject({ x: 42, y: 43 });
    const fresh = snapshot(V, {
      revision: 10,
      pieces: pieces(4).map((p) => ({ ...p, x: p.x + 1 })),
      title: 'Fresh',
    });
    rs.applyServer({ type: 'snapshot', snapshot: fresh });
    expect(rs.snapshot()).toEqual(fresh);
    expect(rs.localDrag()).toBeNull();
    expect(rs.pieces()).toEqual(fresh.pieces); // no stale overlay position survives
    expect(changes[changes.length - 1]?.type).toBe('snapshot');
    // A reconnect's welcome replaces everything too, even at a lower revision
    // (a restarted server).
    rs.beginDrag(0);
    rs.applyServer(welcome(V, { snapshot: snapshot(V, { revision: 1, title: 'After restart' }) }));
    expect(rs.snapshot()!.title).toBe('After restart');
    expect(rs.localDrag()).toBeNull();
  });

  it('moves the whole group with the anchor, ahead of the server echo', () => {
    const { rs } = joined();
    const snap = rs.snapshot()!;
    rs.applyServer({
      type: 'pieces_changed',
      pieces: [
        { ...snap.pieces[0]!, group_id: 0 },
        { ...snap.pieces[1]!, group_id: 0, x: snap.pieces[0]!.x + 50 },
      ],
      revision: 2,
    });
    rs.beginDrag(0);
    expect(rs.localDrag()!.pieceIds.sort()).toEqual([0, 1]);
    rs.dragTo(0, 0);
    const view = rs.pieces();
    expect(view[0]).toMatchObject({ x: 0, y: 0 });
    expect(view[1]).toMatchObject({ x: 50, y: 10 });
    expect(view[2]).toEqual(rs.snapshot()!.pieces[2]); // untouched
    // The authoritative copy is untouched.
    expect(rs.snapshot()!.pieces[0]!.x).toBe(snap.pieces[0]!.x);
  });

  it('is kept while the server says we hold it, cleared when it releases', () => {
    const { rs } = joined();
    rs.beginDrag(2);
    rs.dragTo(10, 10);
    const p = rs.snapshot()!.pieces[2]!;
    rs.applyServer({ type: 'pieces_changed', pieces: [held(p, 'me', 3, 3)], revision: 2 });
    expect(rs.localDrag()?.confirmed).toBe(true);
    expect(rs.pieces()[2]).toMatchObject({ x: 10, y: 10 }); // no rubber-banding to the late echo
    rs.endDrag();
    expect(rs.localDrag()).not.toBeNull(); // waits for the authoritative drop
    rs.applyServer({ type: 'pieces_changed', pieces: [held(p, null, 11, 11)], revision: 3 });
    expect(rs.localDrag()).toBeNull();
    expect(rs.pieces()[2]).toMatchObject({ x: 11, y: 11, held_by: null });
  });

  it('is cleared when someone else holds the piece or a refusal arrives before confirmation', () => {
    const { rs } = joined();
    const p = rs.snapshot()!.pieces[1]!;
    rs.beginDrag(1);
    rs.applyServer({ type: 'pieces_changed', pieces: [held(p, 'bo')], revision: 2 });
    expect(rs.localDrag()).toBeNull();
    rs.beginDrag(1);
    rs.applyServer({ type: 'error', message: "That piece is in Bo's holder" });
    expect(rs.localDrag()).toBeNull();
    // Once confirmed, an unrelated error leaves it alone.
    rs.beginDrag(3);
    rs.applyServer({ type: 'pieces_changed', pieces: [held(rs.snapshot()!.pieces[3]!, 'me')], revision: 3 });
    rs.applyServer({ type: 'error', message: 'You are sending chat messages too quickly. Wait a moment.' });
    expect(rs.localDrag()).not.toBeNull();
  });

  it('is dropped on disconnect and never resumed', () => {
    const { rs } = joined();
    rs.beginDrag(1);
    rs.dragTo(1, 1);
    rs.connectionLost();
    expect(rs.localDrag()).toBeNull();
    expect(rs.cursors.size).toBe(0);
    rs.applyServer(welcome(V));
    expect(rs.localDrag()).toBeNull();
    rs.dragTo(5, 5); // a late pointer move does not revive it
    expect(rs.localDrag()).toBeNull();
  });

  it('attach() wires a socket: messages applied, any non-open state drops the drag', () => {
    let onMsg: ((m: ServerMessage) => void) | null = null;
    let onState: ((s: SocketState, why?: FatalReason) => void) | null = null;
    const rs = new RoomState();
    rs.attach({ onMessage: (cb) => void (onMsg = cb), onState: (cb) => void (onState = cb) });
    onMsg!(welcome(V));
    rs.beginDrag(0);
    onState!('open');
    expect(rs.localDrag()).not.toBeNull();
    onState!('reconnecting');
    expect(rs.localDrag()).toBeNull();
    expect(rs.connected).toBe(false);
  });
});

describe('RoomState presence and chat', () => {
  it('tracks peers and spectators joining and leaving', () => {
    const { rs } = joined(0, { peers: ['Bo'], spectators: [] });
    rs.applyServer({ type: 'peer_joined', name: 'Cy', spectator: false });
    rs.applyServer({ type: 'peer_joined', name: 'Di', spectator: true });
    expect(rs.peers).toEqual(['Bo', 'Cy']);
    expect(rs.spectators).toEqual(['Di']);
    rs.applyServer({ type: 'peer_left', name: 'Bo', spectator: false });
    rs.applyServer({ type: 'peer_left', name: 'Di', spectator: true });
    rs.applyServer({ type: 'peer_left', name: 'Nobody', spectator: false });
    expect(rs.peers).toEqual(['Cy']);
    expect(rs.spectators).toEqual([]);
  });

  it('tracks cursors and forgets hidden ones', () => {
    const { rs } = joined();
    rs.applyServer({ type: 'cursor', client_id: 'bo', name: 'Bo', x: 1, y: 2 });
    expect(rs.cursors.get('bo')).toMatchObject({ name: 'Bo', x: 1, y: 2 });
    rs.applyServer({ type: 'cursor_hidden', client_id: 'bo' });
    expect(rs.cursors.has('bo')).toBe(false);
  });

  it('keeps chat lines, replaces them with chat_history, and caps them', () => {
    const { rs } = joined();
    const line = (id: number): ChatLine => ({ id, client_id: 'bo', name: 'Bo', text: `t${id}`, sent_at_ms: id });
    rs.applyServer({ type: 'chat', ...line(1) });
    rs.applyServer({ type: 'chat', ...line(2) });
    expect(rs.chat).toEqual([line(1), line(2)]);
    rs.applyServer({ type: 'chat_history', lines: [line(1), line(2), line(3)] });
    expect(rs.chat).toEqual([line(1), line(2), line(3)]);
    for (let i = 4; i < CHAT_KEEP + 20; i++) rs.applyServer({ type: 'chat', ...line(i) });
    expect(rs.chat).toHaveLength(CHAT_KEEP);
    expect(rs.chat[rs.chat.length - 1]!.id).toBe(CHAT_KEEP + 19);
    // A reconnect's welcome keeps the lines (a history follows only while chat is on).
    rs.applyServer(welcome(V));
    expect(rs.chat).toHaveLength(CHAT_KEEP);
  });
});
