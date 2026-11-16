import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { protocol_version } from '../../wasm/pieceoff_web_wasm.js';
import { mountBoard, type BoardHandle, type RendererLike } from '../app/board';
import { BUILTIN_MEDIA } from '../app/create';
import { Camera } from '../gl/camera';
import type { Mesh } from '../gl/mesh';
import { RoomState } from '../net/roomState';
import { snapshot as baseSnapshot, welcome } from '../net/testkit';
import type { ClientMessage, PieceHolder, RoomSnapshot, ServerMessage } from '../protocol';
import { initWasm } from '../test-wasm';
import { MAX_CHAT_CODE_POINTS, clampCodePoints } from './chat';
import { inviteLink, reportMailto } from './hostmenu';
import { mountToolbar, type ToolbarHandle } from './toolbar';

let V: number;
beforeAll(async () => {
  await initWasm();
  V = protocol_version();
});

// ---- fakes (a lean copy of board.test.ts's) ------------------------------------

class FakeRenderer implements RendererLike {
  contextLost = false;
  constructor(readonly camera: Camera) {}
  setPuzzle(_m: Mesh, _s: RoomSnapshot): void {}
  setPiece(): void {}
  pieceDepth(id: number): number {
    return id / 1000;
  }
  requestRender(): void {}
  onContextLost(): void {}
  resize(): void {}
  dispose(): void {}
}

function clock() {
  let now = 0;
  let next = 1;
  const pending = new Map<number, { at: number; fn: () => void }>();
  const frames = new Map<number, FrameRequestCallback>();
  const c = {
    get now() {
      return now;
    },
    timers: {
      set(fn: () => void, ms: number) {
        const h = next++;
        pending.set(h, { at: now + ms, fn });
        return h;
      },
      clear(h: unknown) {
        pending.delete(h as number);
      },
    },
    raf: (cb: FrameRequestCallback) => {
      const h = next++;
      frames.set(h, cb);
      return h;
    },
    caf: (h: number) => void frames.delete(h),
    flushFrames() {
      const l = [...frames.values()];
      frames.clear();
      for (const cb of l) cb(now);
    },
    advance(ms: number) {
      const end = now + ms;
      for (;;) {
        let best: [number, { at: number; fn: () => void }] | null = null;
        for (const e of pending) if (e[1].at <= end && (!best || e[1].at < best[1].at)) best = e;
        if (!best) break;
        pending.delete(best[0]);
        now = best[1].at;
        best[1].fn();
      }
      now = end;
      c.flushFrames();
    },
  };
  return c;
}

function ctx2d() {
  const calls: [string, unknown[]][] = [];
  const target: Record<string, unknown> = {};
  const ctx = new Proxy(target, {
    get(t, k) {
      if (typeof k !== 'string') return undefined;
      if (k in t) return t[k];
      if (k === 'measureText') return (s: string) => ({ width: s.length * 6 });
      return (...a: unknown[]) => void calls.push([k, a]);
    },
    set(t, k, v) {
      t[k as string] = v;
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
  return {
    ctx,
    calls,
    texts: () => calls.filter((x) => x[0] === 'fillText').map((x) => x[1][0] as string),
    arcs: () => calls.filter((x) => x[0] === 'arc').length,
  };
}

interface Opts {
  snap?: Partial<RoomSnapshot>;
  welcomeOver?: Record<string, unknown>;
  host?: boolean;
  side?: boolean;
  site?: string;
  clipboard?: { writeText(t: string): Promise<void> } | null;
  prompt?: (msg: string, value: string) => void;
}

let mounted: { board: BoardHandle; tb: ToolbarHandle } | null = null;

function setup(o: Opts = {}) {
  const c = clock();
  const sent: ClientMessage[] = [];
  const state = new RoomState({ now: () => c.now });
  const snap = baseSnapshot(V, {
    media_sha256: BUILTIN_MEDIA[0]!.sha256,
    media_extension: 'mp4',
    host_id: o.host ? 'me' : 'host',
    ...o.snap,
  });
  state.applyServer(welcome(V, { snapshot: snap, peers: ['Bob'], ...o.welcomeOver }));
  const root = document.createElement('div');
  document.body.append(root);
  const over = ctx2d();
  const under = ctx2d();
  const socket = { send: (m: ClientMessage) => (sent.push(m), true), onState: () => () => undefined };
  const board = mountBoard(root, {
    socket,
    state,
    media: { assetBase: '/play-app/' },
    camera: new Camera(800, 600),
    createRenderer: (_c, _m, camera) => new FakeRenderer(camera),
    now: () => c.now,
    timers: c.timers,
    raf: c.raf,
    caf: c.caf,
    matchMedia: null,
    context2d: (cv) => (cv.classList.contains('po-board-under') ? under.ctx : over.ctx),
  });
  const leave = vi.fn();
  const tb = mountToolbar(board, {
    state,
    socket,
    site: o.site ?? 'https://pieceoff.app',
    leave,
    isSide: () => o.side ?? false,
    clipboard: o.clipboard === undefined ? null : o.clipboard,
    prompt: o.prompt ?? (() => undefined),
    timers: c.timers,
    now: () => c.now,
    visualViewport: null,
  });
  mounted = { board, tb };
  const q = <T extends Element = HTMLElement>(sel: string) => root.querySelector<T & Element>(sel) as unknown as T;
  const qa = <T extends Element = HTMLElement>(sel: string) => [...root.querySelectorAll(sel)] as unknown as T[];
  return {
    c, sent, state, root, over, under, board, tb, leave, q, qa,
    receive: (m: ServerMessage) => state.applyServer(m),
    snap: () => state.snapshot()!,
    tab(name: string) {
      tb.open(name as never);
    },
    hostSent: () => sent.filter((m) => HOST_TYPES.has(m.type)),
  };
}

const HOST_TYPES = new Set(['kick_player', 'set_listed', 'set_chat_enabled', 'restart_puzzle', 'set_playback', 'release_name']);

const chatLine = (id: number, name: string, text: string, client = `c-${name}`) => ({
  type: 'chat' as const, id, client_id: client, name, text, sent_at_ms: 0,
});

function key(el: Element, k: string, shift = false) {
  const e = new KeyboardEvent('keydown', { key: k, shiftKey: shift, bubbles: true, cancelable: true });
  el.dispatchEvent(e);
  return e;
}

function typeInto(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockImplementation(() => Promise.resolve());
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined);
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => undefined);
});
afterEach(() => {
  mounted?.tb.destroy();
  mounted?.board.destroy();
  mounted = null;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

// ---- required ------------------------------------------------------------------

describe('toolbar', () => {
  it('chat_disabled_when_room_chat_off', () => {
    const s = setup({ snap: { chat_enabled: false, listed: true } });
    s.tab('chat');
    const input = s.q<HTMLInputElement>('.po-chat-input');
    const send = s.q<HTMLButtonElement>('.po-chat-send');
    expect(input.disabled).toBe(true);
    expect(send.disabled).toBe(true);
    const note = s.q('.po-chat-off');
    expect(note.hidden).toBe(false);
    expect(note.textContent).toContain('The host turned chat off in this room.');
    expect(note.textContent).toContain('Public rooms');
    // Enter on the (disabled) form sends nothing.
    input.value = 'hi';
    s.q<HTMLFormElement>('.po-chat-form').dispatchEvent(new Event('submit', { cancelable: true }));
    expect(s.sent.filter((m) => m.type === 'chat')).toEqual([]);
    // The host turns it back on: the server's snapshot enables it.
    s.receive({ type: 'snapshot', snapshot: { ...s.snap(), revision: 5, chat_enabled: true } });
    expect(input.disabled).toBe(false);
    expect(note.hidden).toBe(true);
    typeInto(input, 'hi');
    s.q<HTMLFormElement>('.po-chat-form').dispatchEvent(new Event('submit', { cancelable: true }));
    expect(s.sent.filter((m) => m.type === 'chat')).toEqual([{ type: 'chat', text: 'hi' }]);
  });

  it('kick_requires_host_and_confirms', () => {
    // Not the host: no Remove anywhere.
    const g = setup();
    g.tab('people');
    expect(g.qa('[data-act=kick]')).toHaveLength(0);
    g.tb.destroy();
    g.board.destroy();
    document.body.replaceChildren();

    const s = setup({ host: true, welcomeOver: { peers: ['Bob'], spectators: ['Wes'] } });
    s.tab('people');
    const kickBob = () => s.q<HTMLButtonElement>('li[data-name="Bob"] [data-act=kick]');
    expect(kickBob()).toBeTruthy();
    expect(s.q('li[data-name="Wes"] [data-act=kick]')).toBeTruthy(); // spectators can be removed too
    expect(s.q('li[data-name="Ann"] [data-act=kick]')).toBeNull(); // never yourself
    kickBob().click();
    expect(s.hostSent()).toEqual([]);
    const dlg = s.q('.po-confirm');
    expect(dlg.hidden).toBe(false);
    expect(dlg.textContent).toContain('Bob');
    s.q<HTMLButtonElement>('.po-confirm [data-act=cancel]').click();
    expect(dlg.hidden).toBe(true);
    expect(s.hostSent()).toEqual([]);
    kickBob().click();
    s.q<HTMLButtonElement>('.po-confirm [data-act=confirm]').click();
    expect(s.hostSent()).toEqual([{ type: 'kick_player', name: 'Bob' }]);
  });

  it('listed_toggle_shows_builtin_only_error_verbatim', () => {
    const s = setup({ host: true });
    s.tab('room');
    const box = s.q<HTMLInputElement>('input[data-act=listed]');
    expect(box.checked).toBe(false);
    box.checked = true;
    box.dispatchEvent(new Event('change', { bubbles: true }));
    expect(s.hostSent()).toEqual([{ type: 'set_listed', listed: true }]);
    s.receive({ type: 'error', message: 'Public rooms use built-in puzzles only' });
    expect(s.q('.po-panel-status').textContent).toBe('Public rooms use built-in puzzles only');
    // The switch shows the room as it is (the server refused).
    expect(box.checked).toBe(false);
    // The board's toast has it verbatim too.
    expect(s.q('.po-board-toast').textContent).toBe('Public rooms use built-in puzzles only');
  });

  it('credits_sum_matches_piece_count_in_display', () => {
    const pieces = baseSnapshot(V).pieces.map((p) => ({ ...p, placed: true }));
    const s = setup({
      snap: {
        pieces,
        stats: { elapsed_ms: 125_000, running_since_ms: null, started: true, finished: true, players: [{ name: 'Ann', pieces: 3 }, { name: 'Bob', pieces: 1 }] },
      },
    });
    s.tab('people');
    const credits = s.qa('.po-credits li').map((li) => [li.getAttribute('data-name'), Number(li.querySelector('.po-credit-n')!.textContent)]);
    expect(credits).toEqual([['Ann', 3], ['Bob', 1]]);
    const sum = credits.reduce((a, [, n]) => a + (n as number), 0);
    expect(sum).toBe(pieces.length);
    expect(s.q('.po-solve-total').textContent).toBe(`${sum} of ${pieces.length} pieces placed`);
    // Finished: the final time, standing still.
    expect(s.q('.po-solve-clock').textContent).toBe('Solved in 2:05');
    s.c.advance(5000);
    expect(s.q('.po-solve-clock').textContent).toBe('Solved in 2:05');
    // The player rows carry the same credits.
    expect(s.q('.po-people li[data-name="Bob"] .po-credit-n').textContent).toBe('1');
  });

  it('muted_name_hides_chat_cursor_and_pings_locally', () => {
    const s = setup({ welcomeOver: { peers: ['Bob', 'Cy'] } });
    s.receive({ type: 'chat_history', lines: [{ id: 1, client_id: 'c-Bob', name: 'Bob', text: 'old bob', sent_at_ms: 0 }] });
    s.receive(chatLine(2, 'Cy', 'hello from cy'));
    s.receive(chatLine(3, 'Bob', 'new bob'));
    s.receive({ type: 'cursor', client_id: 'c-Bob', name: 'Bob', x: 700, y: 150 });
    s.receive({ type: 'cursor', client_id: 'c-Cy', name: 'Cy', x: 800, y: 150 });
    s.c.flushFrames();
    expect(s.over.texts()).toContain('Bob');
    s.tab('people');
    const sentBefore = s.sent.length;
    s.q<HTMLButtonElement>('li[data-name="Bob"] [data-act=mute]').click();
    // Local only: nothing goes to the server.
    expect(s.sent.length).toBe(sentBefore);
    expect(s.q('li[data-name="Bob"] [data-act=mute]').textContent).toBe('Unmute');
    s.tab('chat');
    const lines = () => s.qa('.po-chat-log li').map((li) => li.textContent);
    expect(lines().join('|')).not.toContain('bob');
    expect(lines().join('|')).toContain('hello from cy');
    // Cursor and ping gone from the over layer; Cy's still drawn.
    s.over.calls.length = 0;
    s.receive({ type: 'ping', client_id: 'c-Bob', name: 'Bob', x: 700, y: 150 });
    s.receive({ type: 'cursor', client_id: 'c-Bob', name: 'Bob', x: 710, y: 150 });
    s.c.flushFrames();
    expect(s.over.texts()).not.toContain('Bob');
    expect(s.over.texts()).toContain('Cy');
    expect(s.over.arcs()).toBe(0);
    // A muted line that arrives later is not shown and not counted.
    s.tb.close();
    s.receive(chatLine(4, 'Bob', 'more bob'));
    expect(s.q('.po-badge').hidden).toBe(true);
    // Unmute: everything comes back.
    s.tab('people');
    s.q<HTMLButtonElement>('li[data-name="Bob"] [data-act=mute]').click();
    s.tab('chat');
    expect(lines().filter((t) => t!.includes('bob'))).toHaveLength(3);
    s.over.calls.length = 0;
    s.receive({ type: 'ping', client_id: 'c-Bob', name: 'Bob', x: 700, y: 150 });
    s.c.flushFrames();
    expect(s.over.arcs()).toBeGreaterThan(0);
  });

  // ---- more ----------------------------------------------------------------------

  it('non_host_never_sees_the_host_menu_and_cannot_send_host_messages', () => {
    const s = setup();
    s.tab('room');
    expect(s.q('.po-host')).toBeNull();
    expect(s.qa('[data-act=listed], [data-act=chat-enabled], [data-act=restart], [data-act=playback], [data-act=release]')).toHaveLength(0);
    // Read-only room facts are there for everyone.
    expect(s.q('.po-cut-style').textContent).toContain('Jigsaw');
    s.tb.destroy();
    s.board.destroy();
    document.body.replaceChildren();

    // Host with a confirm open, then the host role moves away: the confirm sends nothing.
    const h = setup({ host: true });
    h.tab('room');
    h.q<HTMLButtonElement>('[data-act=restart]').click();
    expect(h.q('.po-confirm').hidden).toBe(false);
    const confirmBtn = h.q<HTMLButtonElement>('.po-confirm [data-act=confirm]');
    h.receive({ type: 'snapshot', snapshot: { ...h.snap(), revision: 9, host_id: 'someone-else' } });
    confirmBtn.click();
    expect(h.hostSent()).toEqual([]);
    expect(h.q('.po-host')).toBeNull();
  });

  it('restart_confirms_then_sends', () => {
    const s = setup({ host: true });
    s.tab('room');
    s.q<HTMLButtonElement>('[data-act=restart]').click();
    expect(s.hostSent()).toEqual([]);
    s.q<HTMLButtonElement>('.po-confirm [data-act=confirm]').click();
    expect(s.hostSent()).toEqual([{ type: 'restart_puzzle' }]);
  });

  it('host_chat_switch_and_playback_pause', () => {
    const s = setup({
      host: true,
      snap: {
        queue: [{ sha256: BUILTIN_MEDIA[0]!.sha256, extension: 'mp4', title: 't', width: 1, height: 1, duration_ms: 10_000, available: true, position_ms: 0, loop_start_ms: 0, loop_end_ms: 0 }],
        playback: { playing: true, position_ms: 1000, rate: 1, loop_start_ms: 0, loop_end_ms: 0, changed_at_ms: 0 },
      },
      welcomeOver: { server_now_ms: 2500 },
    });
    s.tab('room');
    const chat = s.q<HTMLInputElement>('input[data-act=chat-enabled]');
    expect(chat.checked).toBe(true);
    chat.checked = false;
    chat.dispatchEvent(new Event('change', { bubbles: true }));
    const pb = s.q<HTMLButtonElement>('[data-act=playback]');
    expect(pb.textContent).toBe('Pause');
    pb.click();
    expect(s.hostSent()).toEqual([
      { type: 'set_chat_enabled', enabled: false },
      { type: 'set_playback', playing: false, position_ms: 3500, rate: 1, loop_start_ms: 0, loop_end_ms: 0 },
    ]);
  });

  it('release_name_for_an_absent_holder_owner', () => {
    const away: PieceHolder = { id: 4, owner: 'Gone', label: 'Sky', x: -900, y: 0, width: 200, height: 200, owner_away_since_ms: 5 };
    const s = setup({ host: true, snap: { holders: [away] } });
    s.tab('room');
    s.q<HTMLButtonElement>('li[data-name="Gone"] [data-act=release]').click();
    s.q<HTMLInputElement>('input[data-act=release-input]').value = '  Pat  ';
    s.q<HTMLButtonElement>('[data-act=release-typed]').click();
    expect(s.hostSent()).toEqual([
      { type: 'release_name', name: 'Gone' },
      { type: 'release_name', name: 'Pat' },
    ]);
  });

  it('chat_limit_counts_code_points_including_emoji', () => {
    expect(MAX_CHAT_CODE_POINTS).toBe(300);
    expect(clampCodePoints('😀'.repeat(301), 300)).toBe('😀'.repeat(300));
    const s = setup();
    s.tab('chat');
    const input = s.q<HTMLInputElement>('.po-chat-input');
    typeInto(input, 'a'.repeat(299) + '😀😀');
    // 😀 is two UTF-16 units but one code point (like core's chars()).
    expect([...input.value]).toHaveLength(300);
    expect(input.value.endsWith('a😀')).toBe(true);
    expect(s.q('.po-chat-count').textContent).toBe('300/300');
    key(input, 'Enter');
    s.q<HTMLFormElement>('.po-chat-form').dispatchEvent(new Event('submit', { cancelable: true }));
    const chats = s.sent.filter((m): m is Extract<ClientMessage, { type: 'chat' }> => m.type === 'chat');
    expect(chats).toHaveLength(1);
    expect([...chats[0]!.text]).toHaveLength(300);
    expect(input.value).toBe('');
    // Blank lines are not sent.
    typeInto(input, '   ');
    s.q<HTMLFormElement>('.po-chat-form').dispatchEvent(new Event('submit', { cancelable: true }));
    expect(s.sent.filter((m) => m.type === 'chat')).toHaveLength(1);
  });

  it('unread_badge_counts_lines_while_chat_is_closed', () => {
    const s = setup();
    const badge = s.q('.po-badge');
    expect(badge.hidden).toBe(true);
    s.receive(chatLine(1, 'Bob', 'one'));
    s.receive(chatLine(2, 'Bob', 'two'));
    s.receive(chatLine(3, 'Ann', 'mine', 'me')); // own lines never count
    expect(badge.hidden).toBe(false);
    expect(badge.textContent).toBe('2');
    expect(s.q('[data-open=chat]').getAttribute('aria-label')).toBe('Chat, 2 unread');
    s.tab('chat');
    expect(badge.hidden).toBe(true);
    s.receive(chatLine(4, 'Bob', 'seen'));
    expect(badge.hidden).toBe(true);
    // Another tab in the panel: chat is not visible, so it counts.
    s.tab('people');
    s.receive(chatLine(5, 'Bob', 'missed'));
    expect(badge.textContent).toBe('1');
  });

  it('chat_lines_show_sender_colour_and_text_never_markup', () => {
    const s = setup();
    s.receive(chatLine(1, 'Bob', '<img src=x onerror=alert(1)>'));
    s.tab('chat');
    const li = s.q('.po-chat-log li');
    expect(li.querySelector('img')).toBeNull();
    expect(li.textContent).toContain('<img src=x onerror=alert(1)>');
    const who = li.querySelector<HTMLElement>('.po-chat-name')!;
    expect(who.textContent).toBe('Bob');
    expect(who.style.color).not.toBe('');
  });

  it('invite_link_from_config_origin_with_prompt_fallback', async () => {
    expect(inviteLink('https://example.test/', 'ABCD2345')).toBe('https://example.test/join?c=ABCD2345');
    const writes: string[] = [];
    const s = setup({ site: 'https://example.test', clipboard: { writeText: async (t) => void writes.push(t) } });
    s.tab('room');
    s.q<HTMLButtonElement>('[data-act=copy-invite]').click();
    await Promise.resolve();
    await Promise.resolve();
    expect(writes).toEqual(['https://example.test/join?c=ABCD2345']);
    expect(s.q('.po-panel-status').textContent).toContain('copied');
    s.tb.destroy();
    s.board.destroy();
    document.body.replaceChildren();

    const prompt = vi.fn();
    const f = setup({ site: 'https://example.test', clipboard: { writeText: () => Promise.reject(new Error('no')) }, prompt });
    f.tab('room');
    f.q<HTMLButtonElement>('[data-act=copy-invite]').click();
    await new Promise((r) => setTimeout(r, 0));
    expect(prompt).toHaveBeenCalledWith(expect.any(String), 'https://example.test/join?c=ABCD2345');
  });

  it('report_mailto_is_url_encoded_with_the_code', () => {
    const want = 'mailto:contact@pieceoff.app?subject=Report%20PieceOff%20room%20ABCD2345&body=Room%20code%3A%20ABCD2345%0AWhat%20happened%3A%20';
    expect(reportMailto('ABCD2345')).toBe(want);
    // Anything but letters and digits is dropped (like the desktop).
    expect(reportMailto('AB&CD?2345')).toBe(want);
    const s = setup({ snap: { listed: true } });
    s.tab('room');
    expect(s.q<HTMLAnchorElement>('a[data-act=report]').getAttribute('href')).toBe(want);
    // Private rooms: no report link (desktop: public rooms only).
    s.receive({ type: 'snapshot', snapshot: { ...s.snap(), revision: 3, listed: false } });
    expect(s.q('a[data-act=report]')).toBeNull();
  });

  it('leave_button_leaves', () => {
    const s = setup();
    s.tab('room');
    s.q<HTMLButtonElement>('[data-act=leave]').click();
    expect(s.leave).toHaveBeenCalledTimes(1);
  });

  it('sheet_traps_focus_and_escape_closes', () => {
    const s = setup();
    const opener = s.q<HTMLButtonElement>('[data-open=menu]');
    opener.focus();
    opener.click();
    const panel = s.q('.po-panel');
    expect(panel.hidden).toBe(false);
    expect(panel.getAttribute('aria-modal')).toBe('true');
    expect(panel.contains(document.activeElement)).toBe(true);
    const focusables = [...panel.querySelectorAll<HTMLElement>('button, input, a[href], [tabindex="0"]')].filter((e) => !e.hidden && !(e as HTMLButtonElement).disabled && !e.closest('[hidden]'));
    const first = focusables[0]!;
    const last = focusables[focusables.length - 1]!;
    last.focus();
    let e = key(last, 'Tab');
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
    e = key(first, 'Tab', true);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
    e = key(first, 'Escape');
    expect(e.defaultPrevented).toBe(true);
    expect(panel.hidden).toBe(true);
    expect(document.activeElement).toBe(opener);
  });

  it('side_panel_is_not_modal_and_escape_still_closes', () => {
    const s = setup({ side: true });
    s.q<HTMLButtonElement>('[data-open=chat]').click();
    const panel = s.q('.po-panel');
    expect(panel.hidden).toBe(false);
    expect(panel.getAttribute('aria-modal')).toBeNull();
    expect(s.board.el.classList.contains('has-panel')).toBe(true);
    const input = s.q<HTMLInputElement>('.po-chat-input');
    expect(document.activeElement).toBe(input);
    expect(key(input, 'Tab').defaultPrevented).toBe(false);
    key(input, 'Escape');
    expect(panel.hidden).toBe(true);
    expect(s.board.el.classList.contains('has-panel')).toBe(false);
  });

  it('new_holder_sends_a_core_rect_within_limits_and_blocks_at_four', () => {
    const s = setup();
    s.tab('holders');
    s.q<HTMLButtonElement>('[data-act=new-holder]').click();
    const made = s.sent.filter((m): m is Extract<ClientMessage, { type: 'create_holder' }> => m.type === 'create_holder');
    expect(made).toHaveLength(1);
    const snap = s.snap();
    const cw = snap.width / snap.columns;
    const ch = snap.height / snap.rows;
    const m = made[0]!;
    expect(m.width / cw).toBeGreaterThanOrEqual(2);
    expect(m.width / cw).toBeLessThanOrEqual(12);
    expect(m.height / ch).toBeGreaterThanOrEqual(2);
    expect(m.height / ch).toBeLessThanOrEqual(12);
    expect(m.label).toBe('Holder 1');
    // Clear of the board.
    const clear = m.x + m.width <= 0 || m.y + m.height <= 0 || m.x >= snap.width || m.y >= snap.height;
    expect(clear).toBe(true);

    // Four of mine already: blocked locally with a clear message.
    const mine = (id: number): PieceHolder => ({ id, owner: 'Ann', label: `H${id}`, x: -2000 - id * 300, y: 0, width: 2 * cw, height: 2 * ch });
    s.receive({ type: 'pieces_changed', revision: 4, pieces: [], holders: [mine(1), mine(2), mine(3), mine(4)] });
    s.sent.length = 0;
    s.q<HTMLButtonElement>('[data-act=new-holder]').click();
    expect(s.sent.filter((x) => x.type === 'create_holder')).toEqual([]);
    expect(s.q('.po-panel-status').textContent).toBe('You already have 4 holders. Remove one to make another.');
    // Removing one of mine sends delete_holder.
    s.q<HTMLButtonElement>('.po-holders li[data-id="2"] [data-act=delete-holder]').click();
    expect(s.sent).toEqual([{ type: 'delete_holder', holder_id: 2 }]);
  });

  it('others_holders_cannot_be_removed_unless_host', () => {
    const theirs: PieceHolder = { id: 7, owner: 'Bob', label: 'Sky', x: -900, y: 0, width: 200, height: 200 };
    const s = setup({ snap: { holders: [theirs] } });
    s.tab('holders');
    expect(s.q('.po-holders li[data-id="7"]')).toBeTruthy();
    expect(s.q('.po-holders li[data-id="7"] [data-act=delete-holder]')).toBeNull();
    s.tb.destroy();
    s.board.destroy();
    document.body.replaceChildren();
    const h = setup({ host: true, snap: { holders: [theirs] } });
    h.tab('holders');
    h.q<HTMLButtonElement>('.po-holders li[data-id="7"] [data-act=delete-holder]').click();
    expect(h.sent).toContainEqual({ type: 'delete_holder', holder_id: 7 });
  });

  it('spectators_can_chat_but_get_no_holder_tools', () => {
    const s = setup({ welcomeOver: { spectator: true, name: 'Wes', spectators: [] } });
    s.tab('chat');
    expect(s.q<HTMLInputElement>('.po-chat-input').disabled).toBe(false);
    expect(s.q('[role=tab][data-tab=holders]').hidden).toBe(true);
    s.tab('people');
    expect(s.q('.po-spectators li[data-name="Wes"]')!.textContent).toContain('you');
  });

  it('players_list_marks_host_and_you', () => {
    const s = setup({ host: true, welcomeOver: { peers: ['Bob'], spectators: ['Wes'] } });
    s.tab('people');
    expect(s.q('.po-people li[data-name="Ann"]').textContent).toContain('you, host');
    expect(s.q('.po-spectators li[data-name="Wes"]')).toBeTruthy();
    s.tb.destroy();
    s.board.destroy();
    document.body.replaceChildren();
    // As a guest, the host is found by client id (cursor or chat line).
    const g = setup();
    g.receive({ type: 'cursor', client_id: 'host', name: 'Bob', x: 700, y: 100 });
    g.tab('people');
    expect(g.q('.po-people li[data-name="Bob"] .po-host-badge')).toBeTruthy();
  });

  it('peer_joins_and_leaves_update_the_list', () => {
    const s = setup();
    s.tab('people');
    s.receive({ type: 'peer_joined', name: 'Dee', spectator: false });
    expect(s.q('.po-people li[data-name="Dee"]')).toBeTruthy();
    s.receive({ type: 'peer_left', name: 'Dee', spectator: false });
    expect(s.q('.po-people li[data-name="Dee"]')).toBeNull();
  });

  it('destroy_removes_the_toolbar', () => {
    const s = setup();
    s.tb.destroy();
    expect(s.q('.po-panel')).toBeNull();
    expect(s.q('[data-open=chat]')).toBeNull();
  });
});
