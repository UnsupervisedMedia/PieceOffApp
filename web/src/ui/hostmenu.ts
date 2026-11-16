// The Room tab: invite link, Report room (public rooms), Leave, the room's
// facts, and for the host only the room switches the protocol has (v15):
// listed (`set_listed`), chat (`set_chat_enabled`), restart (`restart_puzzle`,
// confirmed), play/pause of the shared loop (`set_playback`, host-only in
// core), and `release_name` for absent players. The cut style is fixed when a
// room is made (no message changes it), so it is shown read-only. Every rule
// is the server's: refusals come back as `error` and are shown verbatim.
import { targetPositionMs } from '../media/sync';
import type { ClientMessage, CutStyle, RoomSnapshot } from '../protocol';
import { append, button, h, uid } from './dom';
import { patchRows } from './rows';

/** Where public-room reports go (desktop moderation_ui.rs REPORT_ADDRESS). */
export const REPORT_ADDRESS = 'contact@pieceoff.app';

/** `mailto:` for reporting room `code`, the desktop's exact text (moderation_ui.rs `report_mailto`). */
export function reportMailto(code: string): string {
  const c = code.replace(/[^A-Za-z0-9]/g, '');
  const subject = encodeURIComponent(`Report PieceOff room ${c}`);
  const body = encodeURIComponent(`Room code: ${c}\nWhat happened: `);
  return `mailto:${REPORT_ADDRESS}?subject=${subject}&body=${body}`;
}

/** `<site>/join?c=CODE` (the site origin comes from the page's config). */
export function inviteLink(site: string, code: string): string {
  return `${site.replace(/\/+$/, '')}/join?c=${encodeURIComponent(code)}`;
}

export const CUT_STYLE_LABEL: Record<CutStyle, string> = {
  jigsaw: 'Jigsaw',
  hexagon: 'Hexagon',
  triangle: 'Triangle',
};

export interface ConfirmRequest {
  title: string;
  body: string;
  ok: string;
  run(): void;
}

export interface RoomSource {
  snapshot(): RoomSnapshot | null;
  serverNow(): number;
  readonly name: string;
  readonly spectator: boolean;
}

export interface RoomPanelDeps {
  state: RoomSource;
  isHost(): boolean;
  /** Sends only while this client is the host (and connected). */
  sendHost(m: ClientMessage): boolean;
  site: string;
  clipboard: { writeText(t: string): Promise<void> } | null;
  prompt(message: string, value: string): void;
  status(text: string): void;
  confirm(c: ConfirmRequest): void;
  leave(): void;
}

export class RoomPanel {
  readonly el: HTMLElement;
  private readonly facts: HTMLElement;
  private readonly hostBox: HTMLElement;
  private readonly reportBox: HTMLElement;
  private readonly linkText: HTMLElement;
  // The host section is built once and patched: routine room traffic must not
  // recreate its controls (focus, a half-typed name and its caret survive).
  private readonly host: HTMLElement;
  private readonly listedBox: HTMLInputElement;
  private readonly chatBox: HTMLInputElement;
  private readonly playRow: HTMLElement;
  private readonly playBtn: HTMLButtonElement;
  private readonly awayList: HTMLUListElement;
  private readonly releaseInput: HTMLInputElement;
  private readonly report: HTMLElement;
  private readonly reportLink: HTMLAnchorElement;

  constructor(private readonly d: RoomPanelDeps) {
    this.linkText = h('code', { class: 'po-invite-link' });
    this.facts = h('dl', { class: 'po-facts' });
    this.hostBox = h('div', { class: 'po-host-slot' });
    this.reportBox = h('div', { class: 'po-report-slot' });

    const listed = this.toggle('listed', 'Listed publicly', 'Show this room in the lobby so anyone can join. Built-in puzzles only.', (on) =>
      this.d.sendHost({ type: 'set_listed', listed: on }),
    );
    const chat = this.toggle('chat-enabled', 'Room chat', 'Let everyone chat. Public rooms start with chat off; pings always work.', (on) =>
      this.d.sendHost({ type: 'set_chat_enabled', enabled: on }),
    );
    this.listedBox = listed.box;
    this.chatBox = chat.box;
    this.playBtn = button('Pause', () => this.togglePlayback(), { class: 'btn', 'data-act': 'playback' });
    this.playRow = h('div', { class: 'actions' }, this.playBtn);
    this.awayList = h('ul', { class: 'po-list' });
    const inputId = uid('release');
    this.releaseInput = h('input', { id: inputId, class: 'input', type: 'text', 'data-act': 'release-input', placeholder: 'Name', autocomplete: 'off', maxlength: '64' });
    this.host = h(
      'section',
      { class: 'po-host', 'aria-label': 'Host controls' },
      h('h3', { class: 'po-sub' }, 'Host'),
      listed.row,
      chat.row,
      this.playRow,
      h(
        'div',
        { class: 'actions' },
        button(
          'Restart puzzle…',
          () =>
            this.d.confirm({
              title: 'Restart the puzzle?',
              body: 'Every piece goes back to the tray and the solve clock and credits start over, for everyone.',
              ok: 'Restart',
              run: () => this.d.sendHost({ type: 'restart_puzzle' }),
            }),
          { class: 'btn', 'data-act': 'restart' },
        ),
      ),
      h(
        'div',
        { class: 'po-release' },
        h('h3', { class: 'po-sub' }, 'Release a name'),
        h('p', { class: 'muted' }, 'Free an absent player’s name (and their holders) so they can rejoin from another device. Names also free up 10 minutes after their player leaves.'),
        this.awayList,
        h(
          'div',
          { class: 'po-inline' },
          h('label', { class: 'visually-hidden', for: inputId }, 'Name to release'),
          this.releaseInput,
          button('Release', () => {
            this.release(this.releaseInput.value);
            this.releaseInput.value = '';
          }, { class: 'btn', 'data-act': 'release-typed' }),
        ),
      ),
    );
    this.reportLink = h('a', { 'data-act': 'report', class: 'btn btn--quiet', rel: 'noopener' }, 'Report room…');
    this.report = h('p', { class: 'po-report' }, this.reportLink, h('span', { class: 'muted' }, ` Email ${REPORT_ADDRESS} about abuse here; the room code is filled in.`));

    this.el = h(
      'div',
      { class: 'po-room' },
      h(
        'section',
        { class: 'po-share', 'aria-label': 'Invite' },
        h('p', {}, 'Invite link: ', this.linkText),
        h('div', { class: 'actions' }, button('Copy invite link', () => this.copy(), { class: 'btn btn--primary', 'data-act': 'copy-invite' })),
      ),
      this.facts,
      this.hostBox,
      this.reportBox,
      h('div', { class: 'actions po-leave' }, button('Leave room', () => d.leave(), { class: 'btn btn--danger', 'data-act': 'leave' })),
    );
    this.render();
  }

  render(): void {
    const s = this.d.state.snapshot();
    const code = s?.invite_code ?? '';
    this.linkText.textContent = inviteLink(this.d.site, code);
    this.facts.replaceChildren();
    if (s) {
      const fact = (k: string, v: string, cls?: string) => append(this.facts, h('dt', {}, k), h('dd', { class: cls ?? null }, v));
      fact('Room code', code);
      fact('Pieces', `${s.pieces.length} (${s.columns} × ${s.rows})`);
      fact('Cut', `${CUT_STYLE_LABEL[s.cut_style] ?? s.cut_style}, fixed for this room`, 'po-cut-style');
      fact('Public', s.listed ? 'Listed in the lobby' : 'Invite only');
      fact('Chat', s.chat_enabled ? 'On' : 'Off');
    }
    this.renderHost(s);
    const listed = !!s?.listed;
    this.reportLink.href = reportMailto(code);
    if (listed && !this.report.isConnected) this.reportBox.append(this.report);
    else if (!listed && this.report.parentNode) this.report.remove();
  }

  private renderHost(s: RoomSnapshot | null): void {
    const show = !!s && this.d.isHost() && !this.d.state.spectator;
    if (!show || !s) {
      if (this.host.parentNode) this.host.remove();
      return;
    }
    this.listedBox.checked = s.listed;
    this.chatBox.checked = s.chat_enabled;
    const duration = s.queue[s.current]?.duration_ms ?? 0;
    this.playRow.hidden = duration <= 0;
    const playing = s.playback.playing;
    this.playBtn.textContent = playing ? 'Pause' : 'Play';
    this.playBtn.setAttribute('aria-label', playing ? 'Pause the video for everyone' : 'Play the video for everyone');
    // Names of absent players who still hold holders (desktop releasable_names).
    const me = this.d.state.name;
    const away = [...new Set(s.holders.filter((x) => x.owner_away_since_ms != null && x.owner !== me).map((x) => x.owner))].sort();
    patchRows(
      this.awayList,
      away.map((n) => ({
        key: n,
        sig: n,
        build: () => h('li', { 'data-name': n }, h('span', {}, `${n} (away)`), button('Release name', () => this.release(n), { class: 'btn btn--quiet', 'data-act': 'release' })),
      })),
    );
    this.awayList.hidden = away.length === 0;
    if (!this.host.parentNode) this.hostBox.append(this.host);
  }

  private togglePlayback(): void {
    const cur = this.d.state.snapshot();
    if (!cur) return;
    const p = cur.playback;
    const dur = cur.queue[cur.current]?.duration_ms ?? 0;
    this.d.sendHost({
      type: 'set_playback',
      playing: !p.playing,
      position_ms: Math.round(targetPositionMs(p, this.d.state.serverNow(), dur)),
      rate: p.rate,
      loop_start_ms: p.loop_start_ms,
      loop_end_ms: p.loop_end_ms,
    });
  }

  /** A switch that always shows the room as the server says; a change asks the server. */
  private toggle(act: string, label: string, hint: string, change: (on: boolean) => void): { row: HTMLElement; box: HTMLInputElement } {
    const id = uid(act);
    const box = h('input', { id, type: 'checkbox', 'data-act': act, 'aria-describedby': `${id}-hint` });
    box.addEventListener('change', () => {
      const want = box.checked;
      box.checked = !want; // the room as it is until the server's snapshot says otherwise
      change(want);
    });
    return { row: h('div', { class: 'check' }, box, h('label', { for: id }, label), h('span', { id: `${id}-hint`, class: 'visually-hidden' }, hint)), box };
  }

  private release(raw: string): void {
    const name = raw.trim();
    if (!name) return;
    if (this.d.sendHost({ type: 'release_name', name })) this.d.status(`Asked to release the name ${name}.`);
  }

  private copy(): void {
    const code = this.d.state.snapshot()?.invite_code ?? '';
    const link = inviteLink(this.d.site, code);
    const fallback = () => this.d.prompt('Copy this invite link:', link);
    const cb = this.d.clipboard;
    if (!cb) {
      fallback();
      return;
    }
    cb.writeText(link).then(
      () => this.d.status('Invite link copied. Anyone with it can join.'),
      () => fallback(),
    );
  }
}
