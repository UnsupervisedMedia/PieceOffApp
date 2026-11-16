import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { initWasm } from '../test-wasm';
import type { LobbyRoom } from '../app/lobby';
import { LobbyError } from '../app/lobby';
import { PIECE_PRESETS } from '../app/create';
import type { FatalReason } from '../net/socket';
import { Banner } from './banner';
import { CreateForm } from './createForm';
import { END_COPY, renderEnd, type EndState } from './fatal';
import { LandingScreen } from './landing';
import { renderNamePrompt } from './namePrompt';

beforeAll(async () => {
  await initWasm();
});
afterEach(() => {
  document.body.replaceChildren();
});

const EVIL = '<img src=x onerror="window.__pwned=1">';
const room = (over: Partial<LobbyRoom> = {}): LobbyRoom => ({
  code: 'ABCD2345',
  title: 'Tidal Glass',
  host_name: 'Ann',
  players: 2,
  max_players: 8,
  spectators: 1,
  pieces: 48,
  placed: 12,
  cut_style: 'hexagon',
  created_at: 1,
  ...over,
});

const handlers = () => ({
  quickMatch: vi.fn(),
  create: vi.fn(),
  enterCode: vi.fn(),
  join: vi.fn(),
  refresh: vi.fn(),
});

describe('screens', () => {
  it('lobby_rows_show_progress_and_never_parse_markup', () => {
    const on = handlers();
    const s = new LandingScreen(on);
    document.body.append(s.el);
    s.setRooms([room({ title: EVIL, host_name: EVIL }), room({ code: 'WXYZ6789', players: 8 })]);
    const rows = s.el.querySelectorAll('li.room');
    expect(rows).toHaveLength(2);
    expect(s.el.querySelector('img')).toBeNull();
    expect(rows[0]!.textContent).toContain(EVIL);
    expect(rows[0]!.textContent).toContain('2/8');
    expect(rows[0]!.textContent).toContain('12/48');
    expect(rows[0]!.textContent).toContain('Hexagons');
    expect(rows[0]!.querySelector('[role=progressbar]')!.getAttribute('aria-valuenow')).toBe('25');
    // A full room offers only Watch.
    const fullButtons = [...rows[1]!.querySelectorAll('button')].map((b) => b.textContent);
    expect(fullButtons).toEqual(['Watch']);
    const openButtons = [...rows[0]!.querySelectorAll('button')].map((b) => b.textContent);
    expect(openButtons).toEqual(['Join', 'Watch']);
    (rows[1]!.querySelector('button') as HTMLButtonElement).click();
    expect(on.join).toHaveBeenCalledWith(expect.objectContaining({ code: 'WXYZ6789' }), true);
  });

  it('lobby_status_reports_errors_and_keeps_the_last_list', () => {
    const s = new LandingScreen(handlers());
    s.setError(new LobbyError('network', null));
    expect(s.el.textContent).toContain('Could not reach PieceOff Online');
    s.setRooms([room()]);
    s.setError(new LobbyError('rate-limited', 429));
    expect(s.el.textContent).toContain('Showing the last list');
    expect(s.el.querySelectorAll('li.room')).toHaveLength(1);
  });

  it('code_entry_submits_raw_text', () => {
    const on = handlers();
    const s = new LandingScreen(on);
    document.body.append(s.el);
    const input = s.el.querySelector('input')!;
    input.value = 'abcd 2345';
    s.el.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    expect(on.enterCode).toHaveBeenCalledWith('abcd 2345');
    s.setCodeError('bad');
    expect(input.getAttribute('aria-invalid')).toBe('true');
  });

  it('name_prompt_shows_refusal_verbatim_and_needs_a_name', () => {
    const submit = vi.fn();
    const el = renderNamePrompt({ code: 'ABCD2345', spectator: false, value: 'Ann', problem: EVIL, submit, back: vi.fn() });
    document.body.append(el);
    expect(el.querySelector('[role=alert]')!.textContent).toBe(EVIL);
    expect(el.querySelector('img')).toBeNull();
    const input = el.querySelector('input')!;
    expect(input.labels?.[0]?.textContent).toBe('Your name');
    input.value = '   ';
    el.dispatchEvent(new Event('submit', { cancelable: true }));
    expect(submit).not.toHaveBeenCalled();
    input.value = 'Bob';
    el.dispatchEvent(new Event('submit', { cancelable: true }));
    expect(submit).toHaveBeenCalledWith('Bob');
  });

  it('every_end_state_has_a_way_out', () => {
    const reasons: EndState[] = [
      'update-client',
      'update-server',
      'kicked',
      'banned',
      'room-gone',
      'paused',
      'rate-limited',
      'opened-elsewhere',
      'name-taken',
      'room-not-found',
      'invalid-code',
    ] satisfies Array<FatalReason | 'room-not-found' | 'invalid-code'>;
    expect(Object.keys(END_COPY).sort()).toEqual([...reasons].sort());
    const titles = new Set<string>();
    for (const r of reasons) {
      const a = { reload: vi.fn(), retry: vi.fn(), lobby: vi.fn() };
      const el = renderEnd(r, a, 'ABCD2345');
      const buttons = [...el.querySelectorAll('button')];
      expect(buttons.length, r).toBeGreaterThan(0);
      expect(buttons.some((b) => b.textContent === 'Back to the lobby'), r).toBe(true);
      titles.add(el.querySelector('h1')!.textContent!);
      buttons[0]!.click();
      const called = a.reload.mock.calls.length + a.retry.mock.calls.length + a.lobby.mock.calls.length;
      expect(called, r).toBe(1);
    }
    expect(titles.size).toBe(reasons.length);
    const a = { reload: vi.fn(), retry: vi.fn(), lobby: vi.fn() };
    const upd = renderEnd('update-client', a);
    expect(upd.querySelector('button')!.textContent).toBe('Reload to update');
    upd.querySelector('button')!.click();
    expect(a.reload).toHaveBeenCalled();
  });

  it('create_form_lists_the_eight_builtins_with_posters', () => {
    const submit = vi.fn();
    const f = new CreateForm({ assetBase: '/play-app/', name: 'Ann', listed: true, submit, back: vi.fn() });
    document.body.append(f.el);
    const puzzles = f.el.querySelectorAll<HTMLInputElement>('input[name=puzzle]');
    expect(puzzles).toHaveLength(8);
    const imgs = f.el.querySelectorAll('img');
    expect(imgs).toHaveLength(8);
    expect(imgs[0]!.getAttribute('src')).toMatch(/^\/play-app\/builtin\/[0-9a-f]{64}\.jpg$/);
    const counts = [...f.el.querySelectorAll<HTMLInputElement>('input[name=pieces]')].map((i) => Number(i.value));
    expect(counts).toEqual(PIECE_PRESETS.map((p) => p.count));
    expect(Math.max(...counts)).toBe(400);
    expect(f.el.querySelectorAll('input[name=cut]')).toHaveLength(3);

    // No token, no create.
    const btn = f.el.querySelector<HTMLButtonElement>('button[type=submit]')!;
    expect(btn.disabled).toBe(true);
    f.el.dispatchEvent(new Event('submit', { cancelable: true }));
    expect(submit).not.toHaveBeenCalled();
    f.setToken('tok');
    expect(btn.disabled).toBe(false);
    puzzles[3]!.checked = true;
    f.el.dispatchEvent(new Event('submit', { cancelable: true }));
    expect(submit).toHaveBeenCalledWith(
      expect.objectContaining({ sha: puzzles[3]!.value, title: 'Dune Light', pieces: 48, cutStyle: 'jigsaw', listed: true, name: 'Ann' }),
    );
    f.setBusy(true);
    expect(btn.disabled).toBe(true);
    f.setBusy(false);
    f.cooldown(60_000);
    expect(btn.disabled).toBe(true);
    f.destroy();
  });

  it('banner_shows_text_not_markup', () => {
    const b = new Banner();
    document.body.append(b.el);
    expect(b.el.hidden).toBe(true);
    b.show(EVIL, 'error');
    expect(b.el.hidden).toBe(false);
    expect(b.el.querySelector('img')).toBeNull();
    expect(b.message).toBe(EVIL);
    b.el.querySelector('button')!.click();
    expect(b.el.hidden).toBe(true);
  });
});
