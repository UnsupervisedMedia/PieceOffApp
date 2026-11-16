// The room toolbar (lazy board chunk): two HUD buttons (Chat, with an unread
// badge, and Menu) open one panel with four tabs: Chat, People, Holders and
// Room. On phones (portrait) the panel is a modal bottom sheet (focus trapped,
// Escape or Close shuts it, it rides above the on-screen keyboard through
// `visualViewport`); on wide screens and landscape phones it is a side panel
// beside the board, which shrinks to make room instead of being covered.
//
// The toolbar never applies a rule itself: it sends messages and draws what
// the server says. Host messages go out only while this client is the host
// (`snapshot.host_id`), and a server `error` that follows a toolbar action is
// shown verbatim in the panel's status line (the board toasts it as well).
// Mute is local: those names' chat lines, cursors and pings are hidden here.
import type { BoardHandle } from '../app/board';
import type { RoomState } from '../net/roomState';
import type { ClientMessage, ServerMessage } from '../protocol';
import { ChatPanel } from './chat';
import { button, h, uid } from './dom';
import { HoldersPanel, type HolderRect } from './holders';
import { RoomPanel, type ConfirmRequest } from './hostmenu';
import { PlayersPanel } from './players';

/** Wide screens and short landscape screens get a side panel, not a sheet (play.css matches). */
export const SIDE_QUERY = '(min-width: 900px), (orientation: landscape) and (max-height: 520px)';
/** An `error` this soon after a toolbar action is taken as its answer. */
export const ACTION_ERROR_WINDOW_MS = 3000;
const STATUS_MS = 8000;

export type Tab = 'chat' | 'people' | 'holders' | 'room';
const TABS: { id: Tab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'people', label: 'People' },
  { id: 'holders', label: 'Holders' },
  { id: 'room', label: 'Room' },
];

const HOST_ONLY = new Set<ClientMessage['type']>(['kick_player', 'set_listed', 'set_chat_enabled', 'restart_puzzle', 'set_playback', 'release_name']);

export interface ToolbarDeps {
  state: RoomState;
  socket: { send(m: ClientMessage): boolean };
  /** Public site origin for invite links (config). */
  site: string;
  leave(): void;
  isSide?: () => boolean;
  clipboard?: { writeText(t: string): Promise<void> } | null;
  prompt?: (message: string, value: string) => void;
  timers?: { set(fn: () => void, ms: number): unknown; clear(h: unknown): void };
  now?: () => number;
  visualViewport?: (EventTarget & { height: number; offsetTop: number }) | null;
  holderRect?: (s: NonNullable<ReturnType<RoomState['snapshot']>>, x: number, y: number) => HolderRect | null;
}

export interface ToolbarHandle {
  open(tab?: Tab): void;
  close(): void;
  readonly isOpen: boolean;
  destroy(): void;
}

type Board = Pick<BoardHandle, 'el' | 'body' | 'hud' | 'clock' | 'camera' | 'setMuted'>;

const FOCUSABLE = 'button, input, select, textarea, a[href], [tabindex="0"]';

export function mountToolbar(board: Board, deps: ToolbarDeps): ToolbarHandle {
  const { state, socket } = deps;
  const now = deps.now ?? (() => Date.now());
  const timers = deps.timers ?? {
    set: (fn: () => void, ms: number) => setTimeout(fn, ms),
    clear: (hd: unknown) => clearTimeout(hd as ReturnType<typeof setTimeout>),
  };
  const isSide =
    deps.isSide ??
    (() => (typeof matchMedia === 'function' ? matchMedia(SIDE_QUERY).matches : false));
  const clipboard =
    deps.clipboard !== undefined ? deps.clipboard : typeof navigator !== 'undefined' && navigator.clipboard ? navigator.clipboard : null;
  const prompt = deps.prompt ?? ((m: string, v: string) => void window.prompt(m, v));
  const cleanups: (() => void)[] = [];

  // ---- roles ----
  const isHost = () => {
    const s = state.snapshot();
    return !!s && state.clientId !== null && s.host_id === state.clientId && !state.spectator;
  };
  const hostName = (): string | null => {
    const s = state.snapshot();
    if (!s) return null;
    if (isHost()) return state.name;
    const c = state.cursors.get(s.host_id);
    if (c) return c.name;
    for (let i = state.chat.length - 1; i >= 0; i--) if (state.chat[i]!.client_id === s.host_id) return state.chat[i]!.name;
    return null;
  };

  // ---- sending ----
  let lastAction = -Infinity;
  const send = (m: ClientMessage): boolean => {
    lastAction = now();
    return socket.send(m);
  };
  const sendHost = (m: ClientMessage): boolean => {
    if (!HOST_ONLY.has(m.type) || !isHost()) {
      status('Only the host can do that.');
      return false;
    }
    return send(m);
  };

  // ---- mute (local) ----
  const muted = new Set<string>();
  const isMuted = (name: string) => muted.has(name.toLowerCase());
  const toggleMute = (name: string) => {
    const k = name.toLowerCase();
    if (muted.has(k)) muted.delete(k);
    else muted.add(k);
    board.setMuted(muted);
    chat.render();
    people.render();
  };

  // ---- panel ----
  const statusEl = h('p', { class: 'po-panel-status', role: 'status', 'aria-live': 'polite' });
  statusEl.hidden = true;
  let statusTimer: unknown = null;
  const status = (text: string) => {
    statusEl.textContent = text;
    statusEl.hidden = false;
    if (statusTimer !== null) timers.clear(statusTimer);
    statusTimer = timers.set(() => {
      statusTimer = null;
      statusEl.hidden = true;
    }, STATUS_MS);
  };

  // Confirm (inline, in the panel): kick and restart.
  let pending: ConfirmRequest | null = null;
  const confirmTitle = h('h3', { class: 'po-sub', id: uid('confirm') });
  const confirmBody = h('p', {});
  const confirmCancel = button('Cancel', () => hideConfirm(), { class: 'btn', 'data-act': 'cancel' });
  const confirmOk = button('OK', () => {
    const p = pending;
    hideConfirm();
    p?.run();
  }, { class: 'btn btn--danger', 'data-act': 'confirm' });
  const confirmEl = h('div', { class: 'po-confirm', role: 'alertdialog', 'aria-labelledby': confirmTitle.id }, confirmTitle, confirmBody, h('div', { class: 'actions' }, confirmCancel, confirmOk));
  confirmEl.hidden = true;
  const showConfirm = (c: ConfirmRequest) => {
    pending = c;
    confirmTitle.textContent = c.title;
    confirmBody.textContent = c.body;
    confirmOk.textContent = c.ok;
    confirmEl.hidden = false;
    confirmCancel.focus();
  };
  const hideConfirm = () => {
    const had = pending !== null;
    pending = null;
    confirmEl.hidden = true;
    if (had && panel.contains(document.activeElement) === false && !panel.hidden) focusIn();
  };

  const chat = new ChatPanel({ state, send, isMuted, isHost });
  const people = new PlayersPanel({
    state,
    clock: board.clock,
    isHost,
    hostName,
    isMuted,
    toggleMute,
    requestKick: (name) =>
      showConfirm({
        title: `Remove ${name}?`,
        body: 'They leave the room now and can’t rejoin it, under this name or another, from this device.',
        ok: 'Remove',
        run: () => {
          if (sendHost({ type: 'kick_player', name })) status(`Removed ${name} from the room.`);
        },
      }),
  });
  const holders = new HoldersPanel({
    state,
    send,
    isHost,
    status,
    rect: deps.holderRect,
    near: () => board.camera.toTable(board.camera.viewW / 2, board.camera.viewH / 2) as [number, number],
  });
  const room = new RoomPanel({ state, isHost, sendHost, site: deps.site, clipboard, prompt, status, confirm: showConfirm, leave: () => deps.leave() });

  const bodies: Record<Tab, HTMLElement> = { chat: chat.el, people: people.el, holders: holders.el, room: room.el };
  const tabBtns = {} as Record<Tab, HTMLButtonElement>;
  const panes = {} as Record<Tab, HTMLElement>;
  const tablist = h('div', { class: 'po-tabs', role: 'tablist', 'aria-label': 'Room panel' });
  const paneBox = h('div', { class: 'po-panes' });
  for (const t of TABS) {
    const id = uid(`tab-${t.id}`);
    const paneId = uid(`pane-${t.id}`);
    const b = button(t.label, () => open(t.id), { class: 'po-tab', role: 'tab', id, 'data-tab': t.id, 'aria-controls': paneId, 'aria-selected': 'false' });
    tabBtns[t.id] = b;
    tablist.append(b);
    const pane = h('div', { class: 'po-pane', role: 'tabpanel', id: paneId, 'aria-labelledby': id }, bodies[t.id]);
    pane.hidden = true;
    panes[t.id] = pane;
    paneBox.append(pane);
  }
  tablist.addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    const shown = TABS.filter((t) => !tabBtns[t.id].hidden);
    const i = shown.findIndex((t) => t.id === tab);
    const next = shown[(i + (e.key === 'ArrowRight' ? 1 : shown.length - 1)) % shown.length]!;
    e.preventDefault();
    open(next.id);
    tabBtns[next.id].focus();
  });
  const closeBtn = button('Close', () => close(), { class: 'btn btn--quiet po-panel-close', 'aria-label': 'Close panel' });
  const panel = h(
    'section',
    { class: 'po-panel', role: 'dialog', 'aria-label': 'Room panel' },
    h('header', { class: 'po-panel-head' }, tablist, closeBtn),
    statusEl,
    confirmEl,
    paneBox,
  );
  panel.hidden = true;
  board.body.append(panel);

  // ---- HUD buttons ----
  const badge = h('span', { class: 'po-badge', 'aria-hidden': 'true' });
  badge.hidden = true;
  const chatBtn = h('button', { type: 'button', class: 'btn po-hud-chat', 'data-open': 'chat', 'aria-label': 'Chat', 'aria-expanded': 'false' }, 'Chat', badge);
  chatBtn.addEventListener('click', () => (isOpen && tab === 'chat' ? close() : open('chat')));
  const menuBtn = button('Menu', () => (isOpen && tab !== 'chat' ? close() : open(lastMenuTab)), { class: 'btn po-hud-menu', 'data-open': 'menu', 'aria-expanded': 'false', 'aria-label': 'People, holders and room' });
  board.hud.append(chatBtn, menuBtn);

  // ---- open / close ----
  let isOpen = false;
  let tab: Tab = 'people';
  let lastMenuTab: Tab = 'people';
  let unread = 0;
  let opener: HTMLElement | null = null;

  const updateBadge = () => {
    badge.hidden = unread === 0;
    badge.textContent = unread > 9 ? '9+' : String(unread);
    chatBtn.setAttribute('aria-label', unread ? `Chat, ${unread} unread` : 'Chat');
  };
  const chatVisible = () => isOpen && tab === 'chat';

  const focusIn = () => {
    if (tab === 'chat' && !chat.input.disabled) chat.input.focus();
    else tabBtns[tab].focus();
  };

  function open(t: Tab = tab): void {
    if (t === 'holders' && tabBtns.holders.hidden) t = 'people';
    if (!isOpen) {
      const a = document.activeElement;
      opener = a instanceof HTMLElement && !panel.contains(a) ? a : null;
    }
    tab = t;
    if (t !== 'chat') lastMenuTab = t;
    for (const x of TABS) {
      tabBtns[x.id].setAttribute('aria-selected', String(x.id === t));
      tabBtns[x.id].classList.toggle('is-on', x.id === t);
      panes[x.id].hidden = x.id !== t;
    }
    const side = isSide();
    panel.classList.toggle('po-panel--side', side);
    panel.classList.toggle('po-panel--sheet', !side);
    if (side) panel.removeAttribute('aria-modal');
    else panel.setAttribute('aria-modal', 'true');
    panel.hidden = false;
    board.el.classList.add('has-panel');
    isOpen = true;
    chatBtn.setAttribute('aria-expanded', String(t === 'chat'));
    menuBtn.setAttribute('aria-expanded', String(t !== 'chat'));
    if (t === 'chat') {
      unread = 0;
      updateBadge();
      chat.render();
    }
    syncViewport();
    focusIn();
  }

  function close(): void {
    if (!isOpen) return;
    hideConfirm();
    isOpen = false;
    panel.hidden = true;
    board.el.classList.remove('has-panel');
    chatBtn.setAttribute('aria-expanded', 'false');
    menuBtn.setAttribute('aria-expanded', 'false');
    const o = opener;
    opener = null;
    if (o && o.isConnected) o.focus();
  }

  panel.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (pending) hideConfirm();
      else close();
      return;
    }
    if (e.key !== 'Tab' || isSide()) return;
    // Sheet: keep focus inside.
    const list = [...panel.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (el) => !el.hidden && !(el as HTMLButtonElement).disabled && !el.closest('[hidden]'),
    );
    if (list.length === 0) return;
    const first = list[0]!;
    const last = list[list.length - 1]!;
    const a = document.activeElement;
    if (e.shiftKey && (a === first || !panel.contains(a))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (a === last || !panel.contains(a))) {
      e.preventDefault();
      first.focus();
    }
  });

  // ---- on-screen keyboard: keep the sheet (and the chat input) above it ----
  const vv = deps.visualViewport !== undefined ? deps.visualViewport : typeof window !== 'undefined' ? window.visualViewport : null;
  const syncViewport = () => {
    if (!vv) return;
    const kb = Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop));
    panel.style.setProperty('--po-kb', `${kb}px`);
    panel.style.setProperty('--po-vvh', `${Math.round(vv.height)}px`);
  };
  if (vv) {
    const onVv = () => {
      syncViewport();
      if (isOpen && document.activeElement === chat.input) chat.input.scrollIntoView?.({ block: 'nearest' });
    };
    vv.addEventListener('resize', onVv);
    vv.addEventListener('scroll', onVv);
    cleanups.push(() => {
      vv.removeEventListener('resize', onVv);
      vv.removeEventListener('scroll', onVv);
    });
  }

  // ---- the room ----
  let shownHost = isHost();
  let shownHostName = hostName();
  const renderAll = () => {
    const host = isHost();
    tabBtns.holders.hidden = state.spectator;
    if (state.spectator && tab === 'holders' && isOpen) open('people');
    if (pending && !host) hideConfirm(); // only the host's actions confirm
    chat.render();
    people.render();
    holders.render();
    room.render();
    shownHost = host;
    shownHostName = hostName();
  };
  renderAll();
  cleanups.push(
    state.onChange((m: ServerMessage | null) => {
      if (!m) return;
      switch (m.type) {
        case 'cursor':
        case 'cursor_hidden':
        case 'ping':
          // Only the host badge can change (a cursor names the host's client).
          if (hostName() !== shownHostName) {
            shownHostName = hostName();
            people.render();
          }
          return;
        case 'chat':
          if (!chatVisible() && m.client_id !== state.clientId && !isMuted(m.name)) {
            unread++;
            updateBadge();
          }
          chat.render();
          return;
        case 'error':
          if (now() - lastAction <= ACTION_ERROR_WINDOW_MS) {
            lastAction = -Infinity;
            status(m.message);
          }
          renderAll();
          return;
        case 'pieces_changed':
          if (m.stats || m.holders) renderAll();
          return;
        default:
          renderAll();
          if (shownHost !== isHost()) renderAll();
      }
    }),
  );

  return {
    open: (t?: Tab) => open(t),
    close,
    get isOpen() {
      return isOpen;
    },
    destroy() {
      for (const c of cleanups.splice(0)) c();
      if (statusTimer !== null) timers.clear(statusTimer);
      people.destroy();
      board.el.classList.remove('has-panel');
      panel.remove();
      chatBtn.remove();
      menuBtn.remove();
    },
  };
}
