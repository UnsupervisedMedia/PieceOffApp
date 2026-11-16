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

  constructor(private readonly d: RoomPanelDeps) {
    this.linkText = h('code', { class: 'po-invite-link' });
    this.facts = h('dl', { class: 'po-facts' });
    this.hostBox = h('div', { class: 'po-host-slot' });
    this.reportBox = h('div', { class: 'po-report-slot' });
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
    this.reportBox.replaceChildren();
    if (s?.listed) {
      this.reportBox.append(
        h(
          'p',
          { class: 'po-report' },
          h('a', { href: reportMailto(code), 'data-act': 'report', class: 'btn btn--quiet', rel: 'noopener' }, 'Report room…'),
          h('span', { class: 'muted' }, ` Email ${REPORT_ADDRESS} about abuse here; the room code is filled in.`),
        ),
      );
    }
  }

  private renderHost(s: RoomSnapshot | null): void {
    const focused = this.hostBox.contains(document.activeElement) ? (document.activeElement as HTMLElement).dataset.act : undefined;
    this.hostBox.replaceChildren();
    if (!s || !this.d.isHost() || this.d.state.spectator) return;
    const box = h('section', { class: 'po-host', 'aria-label': 'Host controls' }, h('h3', { class: 'po-sub' }, 'Host'));

    box.append(
      this.toggle('listed', 'Listed publicly', s.listed, 'Show this room in the lobby so anyone can join. Built-in puzzles only.', (on) =>
        this.d.sendHost({ type: 'set_listed', listed: on }),
      ),
      this.toggle('chat-enabled', 'Room chat', s.chat_enabled, 'Let everyone chat. Public rooms start with chat off; pings always work.', (on) =>
        this.d.sendHost({ type: 'set_chat_enabled', enabled: on }),
      ),
    );

    const item = s.queue[s.current];
    const duration = item?.duration_ms ?? 0;
    if (duration > 0) {
      const pb = s.playback;
      box.append(
        h(
          'div',
          { class: 'actions' },
          button(pb.playing ? 'Pause' : 'Play', () => {
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
          }, { class: 'btn', 'data-act': 'playback', 'aria-label': pb.playing ? 'Pause the video for everyone' : 'Play the video for everyone' }),
        ),
      );
    }

    box.append(
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
    );

    // Names of absent players who still hold holders (desktop releasable_names), plus any typed name.
    const me = this.d.state.name;
    const away = [...new Set(s.holders.filter((x) => x.owner_away_since_ms != null && x.owner !== me).map((x) => x.owner))].sort();
    const rel = h('div', { class: 'po-release' }, h('h3', { class: 'po-sub' }, 'Release a name'));
    rel.append(h('p', { class: 'muted' }, 'Free an absent player’s name (and their holders) so they can rejoin from another device. Names also free up 10 minutes after their player leaves.'));
    if (away.length) {
      rel.append(
        h(
          'ul',
          { class: 'po-list' },
          ...away.map((n) =>
            h('li', { 'data-name': n }, h('span', {}, `${n} (away)`), button('Release name', () => this.release(n), { class: 'btn btn--quiet', 'data-act': 'release' })),
          ),
        ),
      );
    }
    const inputId = uid('release');
    const input = h('input', { id: inputId, class: 'input', type: 'text', 'data-act': 'release-input', placeholder: 'Name', autocomplete: 'off', maxlength: '64' });
    rel.append(
      h(
        'div',
        { class: 'po-inline' },
        h('label', { class: 'visually-hidden', for: inputId }, 'Name to release'),
        input,
        button('Release', () => {
          this.release(input.value);
          input.value = '';
        }, { class: 'btn', 'data-act': 'release-typed' }),
      ),
    );
    box.append(rel);
    this.hostBox.append(box);
    if (focused) box.querySelector<HTMLElement>(`[data-act="${focused}"]`)?.focus();
  }

  private toggle(act: string, label: string, on: boolean, hint: string, change: (on: boolean) => void): HTMLElement {
    const id = uid(act);
    const box = h('input', { id, type: 'checkbox', 'data-act': act, 'aria-describedby': `${id}-hint` });
    box.checked = on;
    box.addEventListener('change', () => {
      const want = box.checked;
      box.checked = on; // shows the room as it is until the server's snapshot says otherwise
      change(want);
    });
    return h('div', { class: 'check' }, box, h('label', { for: id }, label), h('span', { id: `${id}-hint`, class: 'visually-hidden' }, hint));
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
