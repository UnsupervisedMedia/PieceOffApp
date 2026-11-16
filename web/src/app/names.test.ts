// @vitest-environment node
import { beforeAll, describe, expect, it } from 'vitest';
import { initWasm } from '../test-wasm';
import {
  MAX_NAME_CHARS,
  cleanName,
  clearResume,
  loadName,
  loadResume,
  memoryStorage,
  nameAllowed,
  playerKey,
  saveName,
  saveResume,
} from './names';

beforeAll(async () => {
  await initWasm();
});

describe('names', () => {
  it('clean_name_matches_core_sanitize', () => {
    // crates/pieceoff-core/src/sanitize.rs tests, verbatim.
    expect(cleanName('\nAlice\0')).toBe('Alice');
    expect(cleanName(' \t ')).toBe('Guest');
    expect([...cleanName('x'.repeat(100))].length).toBeLessThanOrEqual(24);
    expect([...cleanName('é'.repeat(100))]).toHaveLength(24);
    expect(MAX_NAME_CHARS).toBe(24);
    // 24 characters are taken before trimming, as core does.
    expect(cleanName(' '.repeat(20) + 'abcdefgh')).toBe('abcd');
    // Astral characters count once.
    expect([...cleanName('😀'.repeat(30))]).toHaveLength(24);
  });

  it('public_filter_only_for_listed_rooms', () => {
    expect(nameAllowed('pornstar', true)).toBe(false);
    expect(nameAllowed('pornstar', false)).toBe(true);
    expect(nameAllowed('Scunthorpe', true)).toBe(true);
  });

  it('player_key_is_128_bit_hex_created_once', () => {
    const local = memoryStorage();
    const key = playerKey({ local, session: memoryStorage() });
    expect(key).toMatch(/^[0-9a-f]{32}$/);
    expect(playerKey({ local, session: memoryStorage() })).toBe(key);
    expect(local.getItem('pieceoff.web.playerKey')).toBe(key);
    // A damaged value is replaced, never sent.
    local.setItem('pieceoff.web.playerKey', 'not hex');
    const fresh = playerKey({ local, session: memoryStorage() });
    expect(fresh).toMatch(/^[0-9a-f]{32}$/);
    expect(fresh).not.toBe(key);
  });

  it('player_key_survives_storage_that_throws', () => {
    const broken = {
      getItem: () => {
        throw new Error('SecurityError');
      },
      setItem: () => {
        throw new Error('QuotaExceededError');
      },
      removeItem: () => {
        throw new Error('SecurityError');
      },
    } as unknown as Storage;
    const s = { local: broken, session: broken };
    const a = playerKey(s);
    expect(a).toMatch(/^[0-9a-f]{32}$/);
    expect(playerKey(s)).toBe(a); // kept in memory for the page's life
    expect(loadResume(s, 'ABCD2345')).toBeNull();
    expect(() => saveResume(s, 'ABCD2345', { token: 't', name: 'A', spectator: false })).not.toThrow();
  });

  it('resume_tokens_are_kept_per_room_in_session_storage', () => {
    const s = { local: memoryStorage(), session: memoryStorage() };
    saveResume(s, 'ABCD2345', { token: 'tok', name: 'Ann', spectator: false });
    expect(loadResume(s, 'ABCD2345')).toEqual({ token: 'tok', name: 'Ann', spectator: false });
    expect(loadResume(s, 'WXYZ6789')).toBeNull();
    expect(s.local.length).toBe(0); // never in localStorage
    s.session.setItem('pieceoff.web.resume.WXYZ6789', '{"token":5}');
    expect(loadResume(s, 'WXYZ6789')).toBeNull();
    s.session.setItem('pieceoff.web.resume.WXYZ6789', '{nope');
    expect(loadResume(s, 'WXYZ6789')).toBeNull();
    clearResume(s, 'ABCD2345');
    expect(loadResume(s, 'ABCD2345')).toBeNull();
  });

  it('display_name_is_remembered', () => {
    const s = { local: memoryStorage(), session: memoryStorage() };
    expect(loadName(s)).toBe('');
    saveName(s, 'Ann');
    expect(loadName(s)).toBe('Ann');
  });
});
