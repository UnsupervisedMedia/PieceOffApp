// The client's copy of one room, built only from server-authoritative
// messages, plus one optimistic overlay for the piece(s) this player is
// dragging. No game rules live here: snapping, leases, holders and credits
// are the server's; this applies what it says, the way the desktop client
// does (crates/pieceoff-app/src/net.rs).
import type { ChatLine, Piece, RoomSnapshot, ServerMessage } from '../protocol';
import type { FatalReason, SocketState } from './socket';

/** Chat lines kept on the client (the server keeps 100 for newcomers). */
export const CHAT_KEEP = 200;

export interface LocalDrag {
  anchorId: number;
  pieceIds: number[];
  /** Anchor position (board px). */
  x: number;
  y: number;
  /** The server has said this client holds the anchor. */
  confirmed: boolean;
  /** `drag_end` was sent; waiting for the authoritative drop. */
  ended: boolean;
}

export interface RemoteCursor {
  name: string;
  x: number;
  y: number;
  at: number;
}

interface Overlay extends LocalDrag {
  offsets: Map<number, { dx: number; dy: number }>;
  /** Drawn quarter turns of the held pieces (ahead of the server after a local turn). */
  rotations: Map<number, number>;
}

export class RoomState {
  private snap: RoomSnapshot | null = null;
  private index = new Map<number, number>(); // piece id -> index in snap.pieces
  private drag: Overlay | null = null;
  private readonly cbs: Array<(m: ServerMessage | null) => void> = [];
  private readonly now: () => number;

  clientId: string | null = null;
  name = '';
  spectator = false;
  peers: string[] = [];
  spectators: string[] = [];
  chat: ChatLine[] = [];
  readonly cursors = new Map<string, RemoteCursor>();
  /** Server clock minus local clock, from `welcome.server_now_ms` (0 if unknown). */
  serverClockOffsetMs = 0;
  /** The socket is open (only tracked when `attach`ed). */
  connected = false;

  constructor(o: { now?: () => number } = {}) {
    this.now = o.now ?? (() => Date.now());
  }

  /** The authoritative snapshot (do not mutate). */
  snapshot(): RoomSnapshot | null {
    return this.snap;
  }

  /** Called after every applied change with the message (null for local changes). */
  onChange(cb: (m: ServerMessage | null) => void): () => void {
    this.cbs.push(cb);
    return () => {
      const i = this.cbs.indexOf(cb);
      if (i >= 0) this.cbs.splice(i, 1);
    };
  }

  /** The server's clock now, for the solve clock, chat ages and playback. */
  serverNow(): number {
    return this.now() + this.serverClockOffsetMs;
  }

  /** Feed it from a RoomSocket: messages applied, any non-open state drops the local drag. */
  attach(socket: {
    onMessage(cb: (m: ServerMessage) => void): unknown;
    onState(cb: (s: SocketState, why?: FatalReason) => void): unknown;
  }): void {
    socket.onMessage((m) => this.applyServer(m));
    socket.onState((s) => {
      this.connected = s === 'open';
      if (s !== 'open') this.connectionLost();
    });
  }

  applyServer(m: ServerMessage): void {
    switch (m.type) {
      case 'welcome':
        this.clientId = m.client_id;
        this.name = m.name;
        this.spectator = m.spectator;
        this.peers = [...m.peers];
        this.spectators = [...m.spectators];
        this.serverClockOffsetMs = m.server_now_ms > 0 ? m.server_now_ms - this.now() : 0;
        this.cursors.clear();
        this.replace(m.snapshot); // a reconnect's welcome wins whatever its revision
        break;
      case 'snapshot':
        if (this.snap && m.snapshot.revision < this.snap.revision) return;
        this.replace(m.snapshot);
        break;
      case 'pieces_changed': {
        const snap = this.snap;
        if (!snap || m.revision < snap.revision) return;
        snap.revision = m.revision;
        if (m.stats) snap.stats = m.stats;
        if (m.holders) snap.holders = m.holders;
        for (const p of m.pieces) {
          const i = this.index.get(p.id);
          if (i !== undefined) snap.pieces[i] = p;
        }
        this.settleDrag(m.pieces);
        break;
      }
      case 'playback_changed':
        if (!this.snap || m.revision < this.snap.revision) return;
        this.snap.revision = m.revision;
        this.snap.playback = m.playback;
        break;
      case 'peer_joined':
        (m.spectator ? this.spectators : this.peers).push(m.name);
        break;
      case 'peer_left': {
        const list = m.spectator ? this.spectators : this.peers;
        const i = list.indexOf(m.name);
        if (i >= 0) list.splice(i, 1);
        break;
      }
      case 'cursor':
        if (m.client_id !== this.clientId) {
          this.cursors.set(m.client_id, { name: m.name, x: m.x, y: m.y, at: this.now() });
        }
        break;
      case 'cursor_hidden':
        this.cursors.delete(m.client_id);
        break;
      case 'chat': {
        const { type: _type, ...line } = m;
        this.chat.push(line);
        if (this.chat.length > CHAT_KEEP) this.chat.splice(0, this.chat.length - CHAT_KEEP);
        break;
      }
      case 'chat_history':
        this.chat = m.lines.slice(-CHAT_KEEP);
        break;
      case 'error':
        // A refused drag_start answers with an error and no pieces_changed.
        if (this.drag && !this.drag.confirmed) this.drag = null;
        break;
      default:
        // media_upload_token, ping, timelapse, kicked: events for the UI, no state here.
        break;
    }
    this.emit(m);
  }

  /** The socket dropped: the server expires our leases, so the drag is gone
   *  for good (a reconnect never resumes it), and cursors are stale. */
  connectionLost(): void {
    this.drag = null;
    this.cursors.clear();
    this.emit(null);
  }

  // ---- optimistic local drag ----

  /** Start showing a local drag of `anchorId` (and its group, or the given selection). */
  beginDrag(anchorId: number, pieceIds?: number[]): void {
    const snap = this.snap;
    const anchor = this.piece(anchorId);
    if (!snap || !anchor) return;
    const ids = pieceIds ?? snap.pieces.filter((p) => p.group_id === anchor.group_id).map((p) => p.id);
    if (!ids.includes(anchorId)) ids.push(anchorId);
    const offsets = new Map<number, { dx: number; dy: number }>();
    const rotations = new Map<number, number>();
    for (const id of ids) {
      const p = this.piece(id);
      if (!p) continue;
      offsets.set(id, { dx: p.x - anchor.x, dy: p.y - anchor.y });
      rotations.set(id, p.rotation);
    }
    this.drag = {
      anchorId,
      pieceIds: [...offsets.keys()],
      x: anchor.x,
      y: anchor.y,
      confirmed: false,
      ended: false,
      offsets,
      rotations,
    };
    this.emit(null);
  }

  dragTo(x: number, y: number): void {
    if (!this.drag || this.drag.ended) return;
    this.drag.x = x;
    this.drag.y = y;
    this.emit(null);
  }

  /** `rotate_selection` was sent: turn the held pieces a quarter turn about
   *  the anchor's cell centre now, as core's `rotate_selection` will (and the
   *  desktop's `rotate_origins` does), so the drag stays smooth. The server's
   *  answer then confirms it (see `settleDrag`). */
  turnDrag(turns: 1 | -1): void {
    const d = this.drag;
    const snap = this.snap;
    if (!d || d.ended || !snap) return;
    // Offsets are between top-left corners of equal cells, so turning them
    // about the anchor's centre is turning the vector itself.
    for (const [id, o] of d.offsets) {
      d.offsets.set(id, turns > 0 ? { dx: -o.dy, dy: o.dx } : { dx: o.dy, dy: -o.dx });
      d.rotations.set(id, (((d.rotations.get(id) ?? 0) + turns) % 4 + 4) % 4);
    }
    this.emit(null);
  }

  /** `drag_end` was sent: hold the final spot until the server's drop lands. */
  endDrag(): void {
    if (this.drag) this.drag.ended = true;
  }

  /** Drop the overlay now (cancelled gesture). */
  cancelDrag(): void {
    if (!this.drag) return;
    this.drag = null;
    this.emit(null);
  }

  localDrag(): LocalDrag | null {
    if (!this.drag) return null;
    const { offsets: _offsets, ...rest } = this.drag;
    return { ...rest, pieceIds: [...rest.pieceIds] };
  }

  /** Pieces as they should be drawn: the snapshot with the local drag applied. */
  pieces(): Piece[] {
    const snap = this.snap;
    if (!snap) return [];
    const d = this.drag;
    if (!d) return snap.pieces;
    return snap.pieces.map((p) => {
      const off = d.offsets.get(p.id);
      return off ? { ...p, x: d.x + off.dx, y: d.y + off.dy, rotation: d.rotations.get(p.id) ?? p.rotation } : p;
    });
  }

  piece(id: number): Piece | undefined {
    const i = this.index.get(id);
    return i === undefined ? undefined : this.snap?.pieces[i];
  }

  // ---- internals ----

  private replace(snapshot: RoomSnapshot): void {
    this.snap = snapshot;
    this.index = new Map(snapshot.pieces.map((p, i) => [p.id, i]));
    this.drag = null;
  }

  /** The authoritative word on the anchor decides the overlay: ours → keep (and
   *  mark confirmed); someone else's → gone; free → gone once we had held it
   *  (the drop landed), kept before that (a change that predates our
   *  drag_start; a refusal arrives as an `error`). */
  private settleDrag(changed: Piece[]): void {
    const d = this.drag;
    if (!d) return;
    const anchor = changed.find((p) => p.id === d.anchorId);
    if (!anchor) return;
    if (anchor.held_by !== null && anchor.held_by === this.clientId) {
      d.confirmed = true;
      // Once the server's turn matches ours, take its layout of the held
      // pieces (a turn may have been clamped); until then an older echo would
      // undo a turn it has not seen yet.
      if (anchor.rotation === d.rotations.get(d.anchorId)) {
        for (const p of changed) {
          if (!d.offsets.has(p.id)) continue;
          d.offsets.set(p.id, { dx: p.x - anchor.x, dy: p.y - anchor.y });
          d.rotations.set(p.id, p.rotation);
        }
      }
    } else if (anchor.held_by !== null || d.confirmed) this.drag = null;
  }

  private emit(m: ServerMessage | null): void {
    for (const cb of [...this.cbs]) cb(m);
  }
}
