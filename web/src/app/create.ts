// Creating a hosted room from the browser: `POST <api>/api/rooms` with the
// `create_room` message as the body and the Turnstile token in
// `x-turnstile-token` (worker/src/index.ts). Built-in puzzles only, checked
// here before any request; one request per call, never retried (the Worker
// rate-limits creation per address and caps rooms per day).
import type { ClientMessage, CutStyle } from '../protocol';
import { PUBLIC_NAME_REFUSED, parseCode } from './join';
import { cleanName, nameAllowed, playerKey, browserStorage } from './names';

/** The bundled loops as the desktop app sends them: the canonical `.webp`
 *  (whose sha256 the core lists), its canvas size and its summed frame
 *  durations. A test checks this table against core and the webp files. */
export interface BuiltinMedia {
  sha256: string;
  title: string;
  width: number;
  height: number;
  durationMs: number;
}

const loop = (sha256: string, title: string): BuiltinMedia => ({ sha256, title, width: 480, height: 320, durationMs: 5999 });

/** In core order (pieceoff-core listing.rs BUILTIN_MEDIA_SHA256). */
export const BUILTIN_MEDIA: readonly BuiltinMedia[] = [
  loop('f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80', 'Slow Orbit'),
  loop('06add36908c1cb4f50128d0c9133b38ceea20f873e2d0e1fae06e08795d07da5', 'Tidal Glass'),
  loop('5d5120c8599dcb77e3c3953b65e5abffab114527bd79323b7dd1cdff9902b1b8', 'Aurora Veil'),
  loop('40ecc031c280369707c58927eab00777323fbd6cf6d97cb3a1d831dbcc836eb3', 'Dune Light'),
  loop('5408b1d0f453d263b4a4f5d6983a15655550dd1f45b71ee0b7d0fed0fead64ae', 'Leaf Garden'),
  loop('c98c9143a6e86b3ed06995c144c30e03e9e08cd446b57a45cc996f506cc5cf8a', 'Glass Garden'),
  loop('e0586f46ff010d759130e07aef4b232fe8d8a00efe4dfadf9b663611ba4835f0', 'Deep Sea Bloom'),
  loop('1ea4b43428fb10e9d3c21df0c6f1940a7e6d9207f1fee90d22d249777e2cf00a', 'Geometric Bloom'),
];

export function builtinMedia(sha: string): BuiltinMedia | null {
  const s = typeof sha === 'string' ? sha.toLowerCase() : '';
  return BUILTIN_MEDIA.find((m) => m.sha256 === s) ?? null;
}

// ---- piece counts (desktop ui.rs PIECE_PRESETS / grid_for_piece_count) ----

/** Core's limit (lib.rs: "A room can contain at most 400 pieces"). */
export const MAX_PIECES = 400;
export const MIN_SIDE = 2;
export const MAX_SIDE = 30;

export const PIECE_PRESETS: ReadonlyArray<{ label: string; count: number }> = [
  { label: 'Easy', count: 12 },
  { label: 'Medium', count: 48 },
  { label: 'Hard', count: 108 },
  { label: 'Expert', count: 192 },
  { label: 'Marathon', count: MAX_PIECES },
];

export const CUT_STYLES: ReadonlyArray<{ value: CutStyle; label: string; hint: string }> = [
  { value: 'jigsaw', label: 'Classic', hint: 'Classic jigsaw pieces on a square grid' },
  { value: 'hexagon', label: 'Hexagons', hint: 'Hexagons with knobs; up to six neighbours each' },
  { value: 'triangle', label: 'Triangles', hint: 'Triangles with small knobs; up to three neighbours each' },
];

/** Cell width over height that makes each style's pieces regular (core tiling.rs cell_aspect). */
const CELL_ASPECT: Record<CutStyle, number> = { jigsaw: 1, hexagon: 2 / Math.sqrt(3), triangle: 1 / Math.sqrt(3) };

/** Rows and columns giving about `target` near-regular pieces for media of `aspect` (width / height). */
export function gridForPieceCount(target: number, aspect: number, style: CutStyle = 'jigsaw'): { rows: number; columns: number } {
  const a0 = Number.isFinite(aspect) && aspect > 0 ? aspect : 4 / 3;
  const a = Math.min(10, Math.max(0.1, a0 / CELL_ASPECT[style]));
  const t = Math.min(MAX_PIECES, Math.max(4, Math.round(target) || 4));
  const clampSide = (n: number) => Math.min(MAX_SIDE, Math.max(MIN_SIDE, n));
  let columns = clampSide(Math.round(Math.sqrt(t * a)));
  let rows = clampSide(Math.round(t / columns));
  while (rows * columns > MAX_PIECES) {
    if (columns >= rows) columns--;
    else rows--;
  }
  return { rows, columns };
}

export function validGrid(rows: number, columns: number): boolean {
  const side = (n: number) => Number.isInteger(n) && n >= MIN_SIDE && n <= MAX_SIDE;
  return side(rows) && side(columns) && rows * columns <= MAX_PIECES;
}

/** Core's `clean_title`. */
export function cleanTitle(title: string): string {
  const kept = [...String(title).replace(/\p{Cc}/gu, '')].slice(0, 80).join('').trim();
  return kept === '' ? 'Untitled puzzle' : kept;
}

// ---- errors ----------------------------------------------------------------

export type CreateErrorKind =
  // refused here, before any request
  | 'not-builtin'
  | 'invalid-grid'
  | 'name-not-allowed'
  // refused by the service (status in brackets)
  | 'bad-request' // 400/413: core's own explanation, shown verbatim
  | 'verification' // 403 "Verification failed" (and a missing token, locally)
  | 'forbidden' // other 403/401 (origin, uploads off)
  | 'rate-limited' // 429
  | 'paused' // 503 "PieceOff Online is paused"
  | 'daily-limit' // 503 the daily room cap
  | 'busy' // other 503 (no free code, storage briefly down)
  | 'storage-full' // 507
  | 'server' // anything else
  | 'network' // no answer (offline, DNS, CORS)
  | 'bad-response'; // 200 without a usable body

export class CreateError extends Error {
  constructor(
    readonly kind: CreateErrorKind,
    message: string,
    readonly status: number | null = null,
    /** The service's `error` text, when it sent one. */
    readonly serverText: string | null = null,
  ) {
    super(message);
    this.name = 'CreateError';
  }
}

const tidy = (v: unknown): string | null => {
  if (typeof v !== 'string') return null;
  const t = [...v.replace(/\p{Cc}/gu, ' ')].slice(0, 300).join('').trim();
  return t === '' ? null : t;
};

function refusal(status: number, text: string | null): CreateError {
  const e = (kind: CreateErrorKind, message: string) => new CreateError(kind, message, status, text);
  switch (status) {
    case 400:
    case 413:
      return e('bad-request', text ?? 'PieceOff Online could not make that room.');
    case 401:
      return e('forbidden', 'PieceOff Online refused to create the room from this page.');
    case 403:
      if (text === 'Verification failed') return e('verification', 'The human check failed or expired. Please try it again.');
      return e('forbidden', text ? `PieceOff Online refused to create the room: ${text}` : 'PieceOff Online refused to create the room.');
    case 429:
      return e('rate-limited', 'Too many rooms were created from this connection. Wait a few minutes, then try again.');
    case 503:
      if (text?.startsWith('PieceOff Online is paused')) return e('paused', 'PieceOff Online is paused right now. Please try again later.');
      if (text?.includes("today's limit")) return e('daily-limit', "PieceOff Online has reached today's room limit. Please try again tomorrow.");
      return e('busy', 'PieceOff Online is busy right now. Try again in a moment.');
    case 507:
      return e('storage-full', 'PieceOff Online is out of room space right now. Please try again later.');
    default:
      return e('server', `PieceOff Online had a problem (HTTP ${status}). Please try again later.`);
  }
}

// ---- the call -----------------------------------------------------------------

export interface CreateRoomDeps {
  api: string;
  turnstileToken: string;
  title: string;
  rows: number;
  columns: number;
  sha: string;
  name: string;
  listed: boolean;
  cutStyle?: CutStyle;
  /** Defaults to this browser's stored key. */
  playerKey?: string;
  fetch?: typeof fetch;
  publicNameAllowed?: (name: string, listed: boolean) => boolean;
}

export interface CreatedRoom {
  code: string;
  clientId: string;
  resumeToken: string;
}

export async function createRoom(deps: CreateRoomDeps): Promise<CreatedRoom> {
  const media = builtinMedia(deps.sha);
  if (!media) throw new CreateError('not-builtin', 'PieceOff Online uses the built-in puzzles only. Pick one of those.');
  if (!validGrid(deps.rows, deps.columns)) {
    throw new CreateError('invalid-grid', `Choose between ${MIN_SIDE} and ${MAX_SIDE} rows and columns, at most ${MAX_PIECES} pieces.`);
  }
  const name = cleanName(deps.name);
  const allowed = deps.publicNameAllowed ?? nameAllowed;
  if (!allowed(name, deps.listed)) throw new CreateError('name-not-allowed', PUBLIC_NAME_REFUSED);
  const token = typeof deps.turnstileToken === 'string' ? deps.turnstileToken.trim() : '';
  if (token === '' || token.length > 4096) throw new CreateError('verification', 'Finish the human check first.');

  const message: Extract<ClientMessage, { type: 'create_room' }> = {
    type: 'create_room',
    name,
    title: cleanTitle(deps.title),
    media_sha256: media.sha256,
    media_extension: 'webp',
    width: media.width,
    height: media.height,
    rows: deps.rows,
    columns: deps.columns,
    initial_snapshot: null,
    duration_ms: media.durationMs,
    player_key: deps.playerKey ?? playerKey(browserStorage()),
    cut_style: deps.cutStyle ?? 'jigsaw',
    listed: deps.listed,
  };

  const f = deps.fetch ?? fetch;
  let res: Response;
  try {
    res = await f(`${deps.api}/api/rooms`, {
      method: 'POST',
      credentials: 'omit',
      cache: 'no-store',
      redirect: 'error',
      headers: { 'content-type': 'application/json', 'x-turnstile-token': token },
      body: JSON.stringify(message),
    });
  } catch {
    throw new CreateError('network', 'Could not reach PieceOff Online. Check your connection and try again.');
  }
  let body: unknown = null;
  try {
    body = await res.json();
  } catch {
    body = null;
  }
  const o = body && typeof body === 'object' ? (body as Record<string, unknown>) : {};
  if (!res.ok) throw refusal(res.status, tidy(o.error));
  const code = parseCode(typeof o.code === 'string' ? o.code : null);
  const clientId = typeof o.client_id === 'string' && o.client_id.length <= 128 ? o.client_id : '';
  const resumeToken = typeof o.resume_token === 'string' && o.resume_token.length <= 256 ? o.resume_token : '';
  if (!code || !clientId || !resumeToken) {
    throw new CreateError('bad-response', 'PieceOff Online sent an unexpected answer. Please try again.', res.status);
  }
  return { code, clientId, resumeToken };
}
