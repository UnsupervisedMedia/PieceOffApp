// @vitest-environment node
// The client recognises some server refusals by their text (the protocol has
// no error codes). Every text it matches on must still be what core, the axum
// server or the Worker sends, so a reworded refusal fails here instead of
// silently turning a fatal stop into a reconnect loop (or the reverse).
import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import { initWasm } from '../test-wasm';
import { PUBLIC_NAME_REFUSED, ROOM_FULL } from '../app/join';
import { classifyError } from './socket';

beforeAll(async () => {
  await initWasm();
});

const repo = (p: string) => readFileSync(new URL(`../../../${p}`, import.meta.url), 'utf8');
const sources = {
  core: ['crates/pieceoff-core/src/lib.rs', 'crates/pieceoff-core/src/claims.rs', 'crates/pieceoff-core/src/moderation.rs', 'crates/pieceoff-core/src/membership.rs'].map(repo).join('\n'),
  server: repo('crates/pieceoff-server/src/lib.rs'),
  worker: ['worker/src/index.ts', 'worker/src/room.ts', 'worker/src/registry.ts'].map(repo).join('\n'),
};
const all = Object.values(sources).join('\n');

describe('server texts the client matches on', () => {
  it.each([
    ['ROOM_FULL (join.ts)', ROOM_FULL, sources.core],
    ['PUBLIC_NAME_REFUSED (join.ts)', PUBLIC_NAME_REFUSED, sources.core],
    ['opened elsewhere (socket.ts)', 'This name was opened in another window', sources.core],
    ['opened elsewhere, Worker copy (socket.ts)', 'This name was opened in another window', sources.worker],
    ['name taken (socket.ts)', 'That name is taken in this room', sources.core],
    ['banned (socket.ts)', 'The host removed you from this room', sources.core],
    ['room gone (socket.ts)', 'Room code not found', sources.worker],
    ['paused (socket.ts, create.ts)', 'PieceOff Online is paused', sources.worker],
    ['update (socket.ts)', 'Update PieceOff', sources.core],
    ['not holding (board.ts)', 'You do not hold', sources.core],
    ['daily cap (create.ts)', "today's limit", sources.worker],
    ['Turnstile refusal (create.ts)', '"Verification failed"', sources.worker],
    ['close reason: replaced (socket.ts)', '"replaced"', sources.worker],
    ['close reason: removed by host (socket.ts)', '"removed by host"', sources.worker],
    ['close reason: room expired (socket.ts)', '"room expired"', sources.worker],
  ])('%s is still sent', (_what, text, where) => {
    expect(where.includes(text), text).toBe(true);
  });

  it('the protocol mismatch texts classify as the right update', () => {
    // pieceoff-server builds them with format!("This PieceOff {newer} is newer ...").
    expect(sources.server).toContain('"This PieceOff {newer} is newer');
    expect(classifyError('This PieceOff app is newer (app protocol v17, server protocol v16).')).toBe('update-server');
    expect(classifyError('This PieceOff server is newer (app protocol v15, server protocol v16).')).toBe('update-client');
    expect(all).toContain('This room turns its pieces. Update PieceOff to join it.');
    expect(classifyError('This room turns its pieces. Update PieceOff to join it.')).toBe('update-client');
  });
});
