// Who is in the room: players (you first), then spectators, each with their
// piece credits, a local Mute, and for the host Remove (with a confirm). The
// solve clock and the room's credits (RoomSnapshot.stats) sit on top. Credits
// are display only: the server keeps them, and they add up to the piece count
// once the picture is complete (ARCHITECTURE.md "Multiplayer model").
import { playerColor } from '../gl/overlays';
import type { RoomSnapshot } from '../protocol';
import type { ClockReading, SolveClock } from './clock';
import { button, h } from './dom';
import { patchRows } from './rows';

export interface PeopleSource {
  snapshot(): RoomSnapshot | null;
  readonly name: string;
  readonly spectator: boolean;
  readonly peers: string[];
  readonly spectators: string[];
}

export interface PlayersDeps {
  state: PeopleSource;
  clock: SolveClock;
  isHost(): boolean;
  /** The host's display name when known. */
  hostName(): string | null;
  isMuted(name: string): boolean;
  toggleMute(name: string): void;
  /** Host: ask to remove `name` (the toolbar confirms first). */
  requestKick(name: string): void;
}

export class PlayersPanel {
  readonly el: HTMLElement;
  private readonly clockEl: HTMLElement;
  private readonly total: HTMLElement;
  private readonly credits: HTMLOListElement;
  private readonly people: HTMLUListElement;
  private readonly watchers: HTMLUListElement;
  private readonly watchHead: HTMLElement;
  private readonly off: () => void;

  constructor(private readonly d: PlayersDeps) {
    this.clockEl = h('p', { class: 'po-solve-clock', 'aria-label': 'Solve time' });
    this.total = h('p', { class: 'po-solve-total muted' });
    this.credits = h('ol', { class: 'po-credits', 'aria-label': 'Pieces placed by each player' });
    this.people = h('ul', { class: 'po-people po-list', 'aria-label': 'Players' });
    this.watchHead = h('h3', { class: 'po-sub' }, 'Watching');
    this.watchers = h('ul', { class: 'po-spectators po-list', 'aria-label': 'Watching' });
    this.el = h(
      'div',
      { class: 'po-players' },
      h('section', { class: 'po-solve', 'aria-label': 'Solve' }, this.clockEl, this.total, this.credits),
      h('h3', { class: 'po-sub' }, 'Players'),
      this.people,
      this.watchHead,
      this.watchers,
    );
    this.off = d.clock.onTick((r) => this.showClock(r));
    this.render();
  }

  destroy(): void {
    this.off();
  }

  private showClock(r: ClockReading): void {
    this.clockEl.textContent = r.finished ? `Solved in ${r.text}` : r.started ? r.text : `${r.text} (starts at the first pickup)`;
  }

  render(): void {
    const s = this.d.state.snapshot();
    const st = this.d.state;
    const credit = new Map<string, number>();
    for (const p of s?.stats.players ?? []) credit.set(p.name, p.pieces);
    const placed = s ? s.pieces.filter((p) => p.placed).length : 0;
    const count = s?.pieces.length ?? 0;
    this.total.textContent = `${placed} of ${count} pieces placed`;
    this.credits.replaceChildren(
      ...(s?.stats.players ?? []).map((p) =>
        h('li', { 'data-name': p.name }, dot(p.name), h('span', { class: 'po-credit-who' }, p.name), h('span', { class: 'po-credit-n' }, String(p.pieces))),
      ),
    );
    this.credits.hidden = this.credits.childElementCount === 0;
    this.showClock(this.d.clock.reading);

    const host = this.d.isHost();
    const hostName = this.d.hostName();
    const players = st.spectator ? [...st.peers] : [st.name, ...st.peers];
    const watchers = st.spectator ? [st.name, ...st.spectators] : [...st.spectators];
    // Keyed rows patched in place, so focus on a row's control survives the
    // scoring, joins and snapshots that re-render this panel.
    const rows = (names: string[], meWhen: (n: string) => boolean, hostNameFor: string | null) =>
      names.map((n) => {
        const me = meWhen(n);
        const sig = JSON.stringify([n, me, host, hostNameFor === n, me ? false : this.d.isMuted(n), st.spectator]);
        return { key: n, sig, build: () => this.row(n, me, host, hostNameFor, credit) };
      });
    const extra: Node[] = [];
    if (players.length === 0) extra.push(h('li', { class: 'muted' }, 'Nobody is playing right now.'));
    else if (!st.spectator && st.peers.length === 0) extra.push(h('li', { class: 'muted' }, 'Share the invite link (under Room) to play together.'));
    patchRows(this.people, rows(players, (n) => n === st.name && !st.spectator, hostName), extra);
    patchRows(this.watchers, rows(watchers, (n) => n === st.name && st.spectator, null));
    this.watchHead.hidden = this.watchers.hidden = watchers.length === 0;
    // Credits change with every score: patched into the kept rows.
    for (const li of [...this.people.children, ...this.watchers.children]) {
      const n = (li as HTMLElement).dataset.name;
      const el = li.querySelector('.po-credit-n');
      if (n === undefined || !el) continue;
      const c = String(credit.get(n) ?? 0);
      if (el.textContent !== c) el.textContent = c;
      el.setAttribute('aria-label', `${c} pieces placed`);
    }
  }

  private row(name: string, me: boolean, host: boolean, hostName: string | null, credit: Map<string, number>): HTMLLIElement {
    const isHostRow = me ? host : hostName !== null && hostName === name;
    const label = h('span', { class: 'po-person-name' }, name);
    const muted = !me && this.d.isMuted(name);
    const li = h(
      'li',
      { 'data-name': name, class: muted ? 'is-muted' : null },
      dot(name),
      label,
      me ? h('span', { class: 'muted' }, host ? ' (you, host)' : this.d.state.spectator ? ' (you, watching)' : ' (you)') : null,
      !me && isHostRow ? h('span', { class: 'po-host-badge' }, 'host') : null,
      muted ? h('span', { class: 'muted' }, ' (muted)') : null,
      h('span', { class: 'po-credit-n', title: 'Pieces placed', 'aria-label': `${credit.get(name) ?? 0} pieces placed` }, String(credit.get(name) ?? 0)),
    );
    if (!me) {
      const actions = h('span', { class: 'po-row-actions' });
      actions.append(
        button(muted ? 'Unmute' : 'Mute', () => this.d.toggleMute(name), {
          class: 'btn btn--quiet',
          'data-act': 'mute',
          'aria-pressed': String(muted),
          title: muted ? 'Show their chat, pings and cursor again' : 'Hide their chat, pings and cursor on this device',
        }),
      );
      if (host) {
        actions.append(
          button('Remove…', () => this.d.requestKick(name), {
            class: 'btn btn--quiet btn--danger',
            'data-act': 'kick',
            title: 'Remove from the room. They can’t rejoin it.',
          }),
        );
      }
      li.append(actions);
    }
    return li;
  }
}

function dot(name: string): HTMLElement {
  const d = h('span', { class: 'po-dot', 'aria-hidden': 'true' });
  d.style.background = playerColor(name);
  return d;
}
