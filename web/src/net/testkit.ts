// Test doubles for the network layer: a hand-written WebSocket fake, a fake
// page-visibility target and protocol fixtures built from the generated types.
// Not shipped (nothing outside *.test.ts imports it).
import type { Piece, RoomSnapshot, ServerMessage } from '../protocol';

export interface SentFrame {
  at: number;
  data: string;
}

/** Just enough of `WebSocket` for `RoomSocket`. Every transition is driven by the test. */
export class FakeSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSING = 2;
  readonly CLOSED = 3;

  readyState = 0;
  readonly frames: SentFrame[] = [];
  closedWith: { code?: number; reason?: string } | null = null;
  onopen: ((e: unknown) => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: ((e: { code: number; reason: string; wasClean: boolean }) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;

  constructor(
    readonly url: string,
    private readonly now: () => number = () => Date.now(),
  ) {}

  send(data: string): void {
    if (this.readyState !== 1) throw new Error('InvalidStateError: socket not open');
    this.frames.push({ at: this.now(), data });
  }

  /** Client-initiated close. Real browsers report it asynchronously; RoomSocket
   *  must not care, so this fires the event synchronously. */
  close(code?: number, reason?: string): void {
    if (this.readyState >= 2) return;
    this.closedWith = { code, reason };
    this.readyState = 3;
    this.onclose?.({ code: code ?? 1005, reason: reason ?? '', wasClean: true });
  }

  // ---- server / network side ----
  open(): void {
    this.readyState = 1;
    this.onopen?.({});
  }

  receive(m: ServerMessage | Record<string, unknown> | string): void {
    this.onmessage?.({ data: typeof m === 'string' ? m : JSON.stringify(m) });
  }

  serverClose(code = 1006, reason = ''): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.onclose?.({ code, reason, wasClean: code !== 1006 });
  }

  /** An HTTP-level refusal of the upgrade (403/429/503...): the browser hides
   *  the status and reports error + close 1006 without ever opening. */
  refuse(): void {
    this.onerror?.({});
    this.readyState = 3;
    this.onclose?.({ code: 1006, reason: '', wasClean: false });
  }

  /** Parsed JSON frames (raw text frames such as "ping" are skipped). */
  json(): Array<Record<string, unknown> & { type: string }> {
    const out: Array<Record<string, unknown> & { type: string }> = [];
    for (const f of this.frames) {
      try {
        const v = JSON.parse(f.data);
        if (v && typeof v === 'object') out.push(v);
      } catch {
        // raw frame
      }
    }
    return out;
  }

  ofType(type: string): Array<Record<string, unknown> & { type: string }> {
    return this.json().filter((m) => m.type === type);
  }

  framesOfType(type: string): SentFrame[] {
    return this.frames.filter((f) => {
      try {
        return JSON.parse(f.data).type === type;
      } catch {
        return false;
      }
    });
  }
}

/** Stands in for `document`/`window`: carries `visibilityState` and fires the events. */
export class FakeVisibility extends EventTarget {
  visibilityState: 'visible' | 'hidden' = 'visible';

  hide(): void {
    this.visibilityState = 'hidden';
    this.dispatchEvent(new Event('visibilitychange'));
  }

  /** What a phone does when the tab comes back: visibilitychange, then pageshow. */
  show(): void {
    this.visibilityState = 'visible';
    this.dispatchEvent(new Event('visibilitychange'));
    this.dispatchEvent(new Event('pageshow'));
  }
}

export function pieces(n: number, columns = 2): Piece[] {
  return Array.from({ length: n }, (_, id) => ({
    id,
    group_id: id,
    row: Math.floor(id / columns),
    column: id % columns,
    x: 1000 + id * 10,
    y: 500 + id * 10,
    placed: false,
    held_by: null,
    rotation: 0,
  }));
}

export function snapshot(protocolVersion: number, over: Partial<RoomSnapshot> = {}): RoomSnapshot {
  return {
    protocol_version: protocolVersion,
    id: 'room-1',
    invite_code: 'ABCD2345',
    title: 'Test',
    media_sha256: 'a'.repeat(64),
    media_extension: 'webp',
    media_available: true,
    width: 480,
    height: 320,
    rows: 2,
    columns: 2,
    pieces: pieces(4),
    playback: {
      playing: true,
      position_ms: 0,
      rate: 1,
      loop_start_ms: 0,
      loop_end_ms: 0,
      changed_at_ms: 0,
    },
    host_id: 'host',
    revision: 1,
    shape_seed: 7,
    queue: [],
    current: 0,
    repeat: 'off',
    video_lock: false,
    stats: { elapsed_ms: 0, running_since_ms: null, started: false, finished: false, players: [] },
    holders: [],
    cut_style: 'jigsaw',
    difficulty: { rotation: false, no_hints: false },
    listed: false,
    chat_enabled: true,
    ...over,
  };
}

export function welcome(
  protocolVersion: number,
  over: Partial<Extract<ServerMessage, { type: 'welcome' }>> = {},
  resumeToken = 'token-1',
): Extract<ServerMessage, { type: 'welcome' }> & { resume_token: string } {
  return {
    type: 'welcome',
    client_id: 'me',
    name: 'Ann',
    peers: [],
    snapshot: snapshot(protocolVersion),
    media_upload_token: null,
    spectator: false,
    spectators: [],
    server_now_ms: 0,
    ...over,
    resume_token: resumeToken,
  };
}
