// Landing / lobby: Quick match, Create room, Enter code, and the public room list.
import type { LobbyError, LobbyRoom } from '../app/lobby';
import { hasSeat } from '../app/lobby';
import { button, h, uid } from './dom';

export interface LandingHandlers {
  quickMatch(): void;
  create(): void;
  enterCode(raw: string): void;
  join(room: LobbyRoom, watch: boolean): void;
  refresh(): void;
}

const CUT_LABEL: Record<string, string> = { jigsaw: 'Classic', hexagon: 'Hexagons', triangle: 'Triangles' };

export class LandingScreen {
  readonly el: HTMLElement;
  private readonly list: HTMLUListElement;
  private readonly status: HTMLElement;
  private readonly codeInput: HTMLInputElement;
  private readonly codeError: HTMLElement;
  private readonly quick: HTMLButtonElement;
  private loaded = false;

  constructor(private readonly on: LandingHandlers) {
    const codeId = uid('code');
    const codeErrId = uid('code-err');
    this.codeInput = h('input', {
      id: codeId,
      name: 'code',
      type: 'text',
      inputmode: 'text',
      autocomplete: 'off',
      autocapitalize: 'characters',
      spellcheck: 'false',
      maxlength: 16,
      placeholder: 'ABCD2345',
      'aria-describedby': codeErrId,
      class: 'input input--code',
    });
    this.codeError = h('p', { id: codeErrId, class: 'field-error', role: 'alert' });
    const form = h(
      'form',
      { class: 'code-form', novalidate: true },
      h('label', { for: codeId }, 'Room code'),
      h('div', { class: 'row' }, this.codeInput, h('button', { type: 'submit', class: 'btn' }, 'Join')),
      this.codeError,
    );
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      on.enterCode(this.codeInput.value);
    });

    this.quick = button('Quick match', () => on.quickMatch(), { class: 'btn btn--primary btn--big' });
    this.status = h('p', { class: 'muted lobby__status', role: 'status', 'aria-live': 'polite' }, 'Loading public rooms…');
    this.list = h('ul', { class: 'rooms', 'aria-label': 'Public rooms' });

    this.el = h(
      'section',
      { class: 'screen screen--landing', 'aria-labelledby': 'po-landing-title' },
      h('h1', { id: 'po-landing-title' }, 'Play PieceOff'),
      h(
        'div',
        { class: 'actions actions--stack' },
        this.quick,
        button('Create room', () => on.create(), { class: 'btn btn--big' }),
      ),
      form,
      h(
        'section',
        { class: 'lobby', 'aria-labelledby': 'po-lobby-title' },
        h(
          'div',
          { class: 'lobby__head' },
          h('h2', { id: 'po-lobby-title' }, 'Public rooms'),
          button('Refresh', () => on.refresh(), { class: 'btn btn--quiet' }),
        ),
        this.status,
        this.list,
      ),
    );
  }

  setCodeError(message: string): void {
    this.codeError.textContent = message;
    this.codeInput.setAttribute('aria-invalid', message ? 'true' : 'false');
    if (message) this.codeInput.focus();
  }

  setRooms(rooms: LobbyRoom[]): void {
    this.loaded = true;
    this.list.replaceChildren(...rooms.map((r) => this.row(r)));
    const n = rooms.length;
    this.status.textContent = n ? `${n} public room${n === 1 ? '' : 's'}.` : 'No public rooms right now. Create one.';
  }

  setError(err: LobbyError): void {
    this.status.textContent = this.loaded ? `${err.message} Showing the last list.` : err.message;
  }

  private row(r: LobbyRoom): HTMLLIElement {
    const seat = hasSeat(r);
    const pct = r.pieces ? Math.round((r.placed / r.pieces) * 100) : 0;
    const titleId = uid('room');
    const meta = h('dl', { class: 'room__meta' });
    const add = (k: string, v: string) => meta.append(h('dt', {}, k), h('dd', {}, v));
    if (r.host_name) add('Host', r.host_name);
    add('Players', `${r.players}/${r.max_players}`);
    if (r.spectators) add('Watching', String(r.spectators));
    add('Pieces', r.pieces ? `${r.placed}/${r.pieces}` : '–');
    add('Cut', CUT_LABEL[r.cut_style] ?? 'Classic');

    const bar = h('div', {
      class: 'room__bar',
      role: 'progressbar',
      'aria-label': 'Pieces placed',
      'aria-valuemin': 0,
      'aria-valuemax': 100,
      'aria-valuenow': pct,
    });
    const fill = h('i');
    fill.style.width = `${pct}%`;
    bar.append(fill);

    const actions = h('div', { class: 'actions' });
    if (seat) actions.append(button('Join', () => this.on.join(r, false), { class: 'btn btn--primary', 'aria-describedby': titleId }));
    actions.append(button('Watch', () => this.on.join(r, true), { class: 'btn', 'aria-describedby': titleId }));

    return h(
      'li',
      { class: seat ? 'room' : 'room room--full' },
      h('h3', { id: titleId, class: 'room__title' }, r.title, seat ? null : h('span', { class: 'tag' }, 'Full: watch')),
      meta,
      bar,
      actions,
    );
  }
}
