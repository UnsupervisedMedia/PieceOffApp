// Room chat (ARCHITECTURE.md "Multiplayer model"): `chat_history` then live
// lines, sender coloured from a hash of the display name (like cursors). The
// server tidies, rate-limits and refuses; this panel only keeps the input
// within core's MAX_CHAT_CHARS (counted in code points, like Rust `chars()`),
// shows the host's chat switch, and hides muted names locally.
import { playerColor } from '../gl/overlays';
import type { ChatLine, ClientMessage, RoomSnapshot } from '../protocol';
import { button, h, uid } from './dom';

/** Core `MAX_CHAT_CHARS`: Unicode scalar values, not UTF-16 units. */
export const MAX_CHAT_CODE_POINTS = 300;
/** Show the counter once this close to the limit. */
const COUNT_FROM = 250;

/** The first `max` code points of `s`. */
export function clampCodePoints(s: string, max: number): string {
  let n = 0;
  let i = 0;
  for (const ch of s) {
    if (n === max) return s.slice(0, i);
    n++;
    i += ch.length;
  }
  return s;
}

export const codePointLength = (s: string): number => {
  let n = 0;
  for (const _ of s) n++;
  return n;
};

export interface ChatSource {
  snapshot(): RoomSnapshot | null;
  readonly chat: ChatLine[];
  readonly clientId: string | null;
}

export interface ChatDeps {
  state: ChatSource;
  send(m: ClientMessage): boolean;
  /** Muted names (lower case). */
  isMuted(name: string): boolean;
  /** The host can turn chat back on (shown in the off note). */
  isHost(): boolean;
}

export class ChatPanel {
  readonly el: HTMLElement;
  readonly input: HTMLInputElement;
  private readonly log: HTMLOListElement;
  private readonly sendBtn: HTMLButtonElement;
  private readonly off: HTMLElement;
  private readonly count: HTMLElement;
  private readonly form: HTMLFormElement;
  private shown = 0;

  constructor(private readonly d: ChatDeps) {
    const inputId = uid('chat');
    this.log = h('ol', { class: 'po-chat-log', role: 'log', 'aria-label': 'Chat messages', tabindex: '0' });
    this.off = h('p', { class: 'po-chat-off muted', role: 'note' });
    this.off.hidden = true;
    this.input = h('input', {
      id: inputId,
      class: 'input po-chat-input',
      type: 'text',
      name: 'chat',
      autocomplete: 'off',
      enterkeyhint: 'send',
      placeholder: 'Say something',
      'aria-label': 'Chat message',
    });
    this.count = h('span', { class: 'po-chat-count muted', 'aria-live': 'polite' });
    this.count.hidden = true;
    this.sendBtn = button('Send', () => this.submit(), { class: 'btn btn--primary po-chat-send' });
    this.form = h(
      'form',
      { class: 'po-chat-form', novalidate: true },
      h('label', { class: 'visually-hidden', for: inputId }, 'Chat message'),
      this.input,
      this.count,
      this.sendBtn,
    );
    this.form.addEventListener('submit', (e) => {
      e.preventDefault();
      this.submit();
    });
    this.input.addEventListener('input', () => this.clamp());
    this.el = h('div', { class: 'po-chat' }, this.log, this.off, this.form);
    this.render();
  }

  /** Re-read the room: lines (muted hidden), the chat switch. */
  render(): void {
    const s = this.d.state.snapshot();
    const enabled = s?.chat_enabled ?? true;
    this.input.disabled = !enabled;
    this.sendBtn.disabled = !enabled;
    this.off.hidden = enabled;
    if (!enabled) {
      const parts = ['The host turned chat off in this room.'];
      if (s?.listed) parts.push('Public rooms start with chat off.');
      if (this.d.isHost()) parts.push('You can turn it on under Room.');
      parts.push('Pings still work.');
      this.off.textContent = parts.join(' ');
    }
    const atEnd = this.log.scrollHeight - this.log.scrollTop - this.log.clientHeight < 24;
    const items = this.d.state.chat
      .filter((l) => !this.d.isMuted(l.name))
      .map((l) => this.line(l));
    this.log.replaceChildren(...items);
    if (items.length === 0) this.log.append(h('li', { class: 'po-chat-empty muted' }, 'No messages yet.'));
    if (atEnd || items.length !== this.shown) this.log.scrollTop = this.log.scrollHeight;
    this.shown = items.length;
  }

  private line(l: ChatLine): HTMLLIElement {
    const mine = l.client_id === this.d.state.clientId;
    const who = h('span', { class: 'po-chat-name' }, l.name);
    who.style.color = playerColor(l.name);
    return h('li', { class: mine ? 'po-chat-line is-mine' : 'po-chat-line', 'data-id': l.id }, who, h('span', { class: 'po-chat-text' }, l.text));
  }

  private clamp(): void {
    const v = this.input.value;
    const n = codePointLength(v);
    if (n > MAX_CHAT_CODE_POINTS) {
      this.input.value = clampCodePoints(v, MAX_CHAT_CODE_POINTS);
    }
    const len = Math.min(n, MAX_CHAT_CODE_POINTS);
    this.count.hidden = len < COUNT_FROM;
    this.count.textContent = `${len}/${MAX_CHAT_CODE_POINTS}`;
  }

  private submit(): void {
    if (this.input.disabled) return;
    const text = clampCodePoints(this.input.value, MAX_CHAT_CODE_POINTS).trim();
    if (!text) return;
    if (this.d.send({ type: 'chat', text })) {
      this.input.value = '';
      this.clamp();
    }
  }
}
