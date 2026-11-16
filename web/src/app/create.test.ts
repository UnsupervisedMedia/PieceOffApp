// @vitest-environment node
import { readFileSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { builtin_media_json } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import {
  BUILTIN_MEDIA,
  CreateError,
  MAX_PIECES,
  PIECE_PRESETS,
  createRoom,
  gridForPieceCount,
  type CreateErrorKind,
} from './create';

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
const TIDAL = '06add36908c1cb4f50128d0c9133b38ceea20f873e2d0e1fae06e08795d07da5';
const KEY = '0123456789abcdef0123456789abcdef';

function fetchReturning(status: number, body: unknown) {
  return vi.fn(
    async (_url: string, _init?: RequestInit) =>
      new Response(typeof body === 'string' ? body : JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
  );
}

const base = {
  api: API,
  turnstileToken: 'turnstile-token-1',
  title: 'Tidal Glass',
  rows: 6,
  columns: 8,
  sha: TIDAL,
  name: 'Ann',
  listed: true,
  playerKey: KEY,
};

describe('create room', () => {
  it('create_sends_turnstile_header_and_builtin_sha_only', async () => {
    const f = fetchReturning(200, { code: 'ABCD2345', client_id: 'cid-1', resume_token: 'secret-1' });
    const made = await createRoom({ ...base, sha: TIDAL.toUpperCase(), fetch: f as unknown as typeof fetch });
    expect(made).toEqual({ code: 'ABCD2345', clientId: 'cid-1', resumeToken: 'secret-1' });
    expect(f).toHaveBeenCalledTimes(1);
    const [url, init] = f.mock.calls[0]!;
    expect(url).toBe(`${API}/api/rooms`);
    expect(init!.method).toBe('POST');
    expect(init!.credentials).toBe('omit');
    const headers = new Headers(init!.headers);
    expect(headers.get('x-turnstile-token')).toBe('turnstile-token-1');
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.has('authorization')).toBe(false);
    const body = JSON.parse(init!.body as string);
    expect(body).toEqual({
      type: 'create_room',
      name: 'Ann',
      title: 'Tidal Glass',
      media_sha256: TIDAL,
      media_extension: 'webp',
      width: 480,
      height: 320,
      rows: 6,
      columns: 8,
      initial_snapshot: null,
      duration_ms: 5999,
      player_key: KEY,
      cut_style: 'jigsaw',
      listed: true,
    });

    // Anything that is not one of the eight built-ins never reaches the network.
    const g = fetchReturning(200, {});
    for (const sha of ['a'.repeat(64), '', TIDAL.slice(1), `${TIDAL}00`]) {
      await expect(createRoom({ ...base, sha, fetch: g as unknown as typeof fetch })).rejects.toMatchObject({
        kind: 'not-builtin',
      });
    }
    expect(g).not.toHaveBeenCalled();
  });

  it('create_rejects_bad_input_before_network', async () => {
    const f = fetchReturning(200, {});
    const ff = f as unknown as typeof fetch;
    await expect(createRoom({ ...base, name: 'pornstar', fetch: ff })).rejects.toMatchObject({ kind: 'name-not-allowed' });
    await expect(createRoom({ ...base, rows: 21, columns: 20, fetch: ff })).rejects.toMatchObject({ kind: 'invalid-grid' });
    await expect(createRoom({ ...base, rows: 1, columns: 8, fetch: ff })).rejects.toMatchObject({ kind: 'invalid-grid' });
    await expect(createRoom({ ...base, rows: 2.5, columns: 8, fetch: ff })).rejects.toMatchObject({ kind: 'invalid-grid' });
    await expect(createRoom({ ...base, turnstileToken: '', fetch: ff })).rejects.toMatchObject({ kind: 'verification' });
    expect(f).not.toHaveBeenCalled();
    // An invite-only room does not ask the public filter.
    const g = fetchReturning(200, { code: 'ABCD2345', client_id: 'c', resume_token: 't' });
    await createRoom({ ...base, name: 'pornstar', listed: false, fetch: g as unknown as typeof fetch });
    expect(g).toHaveBeenCalledTimes(1);
  });

  it('create_429_403_507_map_to_distinct_errors_and_no_retry', async () => {
    const cases: Array<[number, unknown, CreateErrorKind]> = [
      [400, { error: 'A room can contain at most 400 pieces' }, 'bad-request'],
      [403, { error: 'Verification failed' }, 'verification'],
      [403, { error: 'Origin not allowed' }, 'forbidden'],
      [413, { error: 'Message too large' }, 'bad-request'],
      [429, { error: 'Too many rooms created. Try again later.' }, 'rate-limited'],
      [503, { error: 'PieceOff Online is paused right now. Please try again later.' }, 'paused'],
      [503, { error: "PieceOff Online has reached today's limit. Please try again tomorrow." }, 'daily-limit'],
      [503, { error: 'Could not pick a free room code. Please try again.' }, 'busy'],
      [507, { error: 'PieceOff Online is out of media space right now.' }, 'storage-full'],
      [500, 'oops', 'server'],
      [200, { code: 'nope' }, 'bad-response'],
    ];
    const messages = new Set<string>();
    const kinds = new Set<string>();
    for (const [status, body, kind] of cases) {
      const f = fetchReturning(status, body);
      const err = await createRoom({ ...base, fetch: f as unknown as typeof fetch }).then(
        () => null,
        (e: unknown) => e,
      );
      expect(err, `${status}`).toBeInstanceOf(CreateError);
      expect((err as CreateError).kind, `${status} ${JSON.stringify(body)}`).toBe(kind);
      expect((err as CreateError).status).toBe(status);
      expect((err as CreateError).message.length).toBeGreaterThan(0);
      messages.add((err as CreateError).message);
      kinds.add(kind);
      // Exactly one request, and nothing scheduled to try again.
      expect(f).toHaveBeenCalledTimes(1);
      expect(vi.getTimerCount()).toBe(0);
    }
    expect(kinds.size).toBe(10);
    expect(messages.size).toBe(cases.length); // every case reads differently
    // The core's own explanation is shown verbatim for a refused room.
    const f = fetchReturning(400, { error: 'A room can contain at most 400 pieces' });
    await expect(createRoom({ ...base, fetch: f as unknown as typeof fetch })).rejects.toMatchObject({
      message: 'A room can contain at most 400 pieces',
    });

    const net = vi.fn(async () => {
      throw new TypeError('Failed to fetch');
    });
    await expect(createRoom({ ...base, fetch: net as unknown as typeof fetch })).rejects.toMatchObject({ kind: 'network' });
    expect(net).toHaveBeenCalledTimes(1);
  });

  it('paused_create_reads_as_paused', async () => {
    const f = fetchReturning(503, { error: 'PieceOff Online is paused right now. Please try again later.' });
    const err = (await createRoom({ ...base, fetch: f as unknown as typeof fetch }).catch((e) => e)) as CreateError;
    expect(err.kind).toBe('paused');
    expect(err.message).toMatch(/paused/i);
  });

  it('builtin_media_table_matches_core_and_webp_files', async () => {
    const core = JSON.parse(builtin_media_json()) as { sha256: string; title: string }[];
    expect(BUILTIN_MEDIA.map((m) => m.sha256).sort()).toEqual(core.map((m) => m.sha256).sort());
    for (const m of BUILTIN_MEDIA) expect(m.title).toBe(core.find((c) => c.sha256 === m.sha256)!.title);
    // Width, height and duration are what the desktop app reads from the same webp.
    const dir = new URL('../../../crates/pieceoff-app/assets/builtin/', import.meta.url);
    const files = readdirSync(dir).filter((f: string) => f.endsWith('.webp'));
    expect(files).toHaveLength(8);
    for (const file of files) {
      const bytes = readFileSync(new URL(encodeURIComponent(file), dir));
      const sha = createHash('sha256').update(bytes).digest('hex');
      const meta = BUILTIN_MEDIA.find((m) => m.sha256 === sha)!;
      expect(meta, file).toBeTruthy();
      expect(webpInfo(bytes), file).toEqual({ width: meta.width, height: meta.height, durationMs: meta.durationMs });
    }
  });

  it('piece_presets_respect_the_400_limit', () => {
    expect(MAX_PIECES).toBe(400);
    expect(PIECE_PRESETS.map((p) => p.count)).toContain(400);
    for (const { count } of PIECE_PRESETS) {
      for (const style of ['jigsaw', 'hexagon', 'triangle'] as const) {
        const { rows, columns } = gridForPieceCount(count, 480 / 320, style);
        expect(rows * columns).toBeLessThanOrEqual(400);
        expect(rows).toBeGreaterThanOrEqual(2);
        expect(columns).toBeLessThanOrEqual(30);
      }
    }
    // Same answers as the desktop's grid_for_piece_count tests.
    expect(gridForPieceCount(12, 4 / 3, 'jigsaw')).toEqual({ rows: 3, columns: 4 });
  });
});

/** Canvas size (VP8X) and summed frame durations (ANMF) of an animated WebP. */
function webpInfo(b: Uint8Array): { width: number; height: number; durationMs: number } {
  const u24 = (o: number) => b[o]! | (b[o + 1]! << 8) | (b[o + 2]! << 16);
  const view = new DataView(b.buffer, b.byteOffset, b.byteLength);
  let width = 0;
  let height = 0;
  let durationMs = 0;
  for (let o = 12; o + 8 <= b.length; ) {
    const id = String.fromCharCode(b[o]!, b[o + 1]!, b[o + 2]!, b[o + 3]!);
    const size = view.getUint32(o + 4, true);
    const p = o + 8;
    if (id === 'VP8X') {
      width = u24(p + 4) + 1;
      height = u24(p + 7) + 1;
    } else if (id === 'ANMF') {
      durationMs += u24(p + 12);
    }
    o = p + size + (size & 1);
  }
  return { width, height, durationMs };
}
