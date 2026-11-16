// Holders: players' private sorting areas (core holders.rs; ARCHITECTURE.md
// "Multiplayer model"). A new one is core's default size at the free table
// spot nearest the middle of the view (`new_holder_rect`, from core's
// `free_holder_spot`), so it starts clear of the board, the tray and other
// holders. Limits are blocked here only to explain them; the server decides.
// Moving and resizing by drag are not in the web client yet.
import { new_holder_rect } from '../../wasm/pieceoff_web_wasm.js';
import { playerColor } from '../gl/overlays';
import type { ClientMessage, PieceHolder, RoomSnapshot } from '../protocol';
import { button, h } from './dom';
import { patchRows } from './rows';

/** Core MAX_HOLDERS_PER_PLAYER / MAX_HOLDERS_PER_ROOM (holders.rs). */
export const MAX_HOLDERS_PER_PLAYER = 4;
export const MAX_HOLDERS_PER_ROOM = 16;

export interface HolderRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function coreHolderRect(s: RoomSnapshot, nearX: number, nearY: number): HolderRect | null {
  try {
    return JSON.parse(new_holder_rect(JSON.stringify(s), nearX, nearY)) as HolderRect | null;
  } catch {
    return null;
  }
}

export interface HoldersSource {
  snapshot(): RoomSnapshot | null;
  readonly name: string;
  readonly spectator: boolean;
}

export interface HoldersDeps {
  state: HoldersSource;
  send(m: ClientMessage): boolean;
  isHost(): boolean;
  /** Board point the new holder should be near (the middle of the view). */
  near(): [number, number];
  status(text: string): void;
  rect?: (s: RoomSnapshot, x: number, y: number) => HolderRect | null;
}

export class HoldersPanel {
  readonly el: HTMLElement;
  private readonly list: HTMLUListElement;
  private readonly newBtn: HTMLButtonElement;

  constructor(private readonly d: HoldersDeps) {
    this.newBtn = button('New holder', () => this.create(), { class: 'btn btn--primary', 'data-act': 'new-holder' });
    this.list = h('ul', { class: 'po-holders po-list', 'aria-label': 'Holders' });
    this.el = h(
      'div',
      { class: 'po-holders-panel' },
      h('div', { class: 'actions' }, this.newBtn),
      h('p', { class: 'po-holders-note muted' }, 'Pieces in your holder are yours to move; others see them but cannot pick them up. A new holder goes on free table near the middle of your view.'),
      this.list,
    );
    this.render();
  }

  private mine(s: RoomSnapshot): PieceHolder[] {
    return s.holders.filter((x) => x.owner === this.d.state.name);
  }

  render(): void {
    const s = this.d.state.snapshot();
    const holders = s?.holders ?? [];
    const host = this.d.isHost();
    const me = this.d.state.name;
    patchRows(
      this.list,
      holders.map((x) => {
        const mine = x.owner === me;
        const sig = JSON.stringify([x.id, x.owner, x.label, x.owner_away_since_ms != null, mine, host]);
        return { key: String(x.id), sig, build: () => this.row(x, mine, host) };
      }),
      holders.length === 0 ? [h('li', { class: 'muted' }, 'No holders yet.')] : [],
    );
  }

  private row(x: PieceHolder, mine: boolean, host: boolean): HTMLLIElement {
    const dot = h('span', { class: 'po-dot', 'aria-hidden': 'true' });
    dot.style.background = playerColor(x.owner);
    const caption = mine ? (x.label || 'Holder') : `${x.label ? `${x.label} · ` : ''}${x.owner}${x.owner_away_since_ms != null ? ' (away)' : ''}`;
    const li = h('li', { 'data-id': x.id }, dot, h('span', {}, caption));
    if (mine || host) {
      li.append(
        button(mine ? 'Remove' : 'Clear', () => this.d.send({ type: 'delete_holder', holder_id: x.id }), {
          class: 'btn btn--quiet',
          'data-act': 'delete-holder',
          title: mine ? 'Remove this holder; its pieces stay where they are' : 'Remove their holder; its pieces stay where they are',
        }),
      );
    }
    return li;
  }

  private create(): void {
    const s = this.d.state.snapshot();
    if (!s || this.d.state.spectator) return;
    const mine = this.mine(s).length;
    if (mine >= MAX_HOLDERS_PER_PLAYER) {
      this.d.status(`You already have ${MAX_HOLDERS_PER_PLAYER} holders. Remove one to make another.`);
      return;
    }
    if (s.holders.length >= MAX_HOLDERS_PER_ROOM) {
      this.d.status(`This room already has ${MAX_HOLDERS_PER_ROOM} holders.`);
      return;
    }
    const [nx, ny] = this.d.near();
    const r = (this.d.rect ?? coreHolderRect)(s, nx, ny);
    if (!r) {
      this.d.status('There is no free space on the table for a holder.');
      return;
    }
    if (this.d.send({ type: 'create_holder', x: r.x, y: r.y, width: r.width, height: r.height, label: `Holder ${mine + 1}` })) {
      this.d.status('New holder: pieces you put in it are yours.');
    }
  }
}
