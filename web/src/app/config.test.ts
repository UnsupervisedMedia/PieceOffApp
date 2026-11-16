import { describe, expect, it } from 'vitest';
import { DEFAULT_CONFIG, readConfig, wsUrl } from './config';

describe('config', () => {
  it('defaults_to_production', () => {
    expect(readConfig({})).toEqual({
      api: 'https://rooms.pieceoff.app',
      turnstileSiteKey: '0x4AAAAAAF7xNWih_NrGxdF_',
      assetBase: '/play-app/',
      site: 'https://pieceoff.app',
    });
    expect(DEFAULT_CONFIG.api).toBe('https://rooms.pieceoff.app');
  });

  it('reads_window_config_and_tidies_it', () => {
    const c = readConfig({
      PIECEOFF_WEB_CONFIG: { api: 'http://127.0.0.1:8787/', turnstileSiteKey: 'site-key', assetBase: '/assets', site: 'http://localhost:4321/' },
    });
    expect(c).toEqual({ api: 'http://127.0.0.1:8787', turnstileSiteKey: 'site-key', assetBase: '/assets/', site: 'http://localhost:4321' });
  });

  it('ignores_unusable_values', () => {
    const c = readConfig({
      PIECEOFF_WEB_CONFIG: { api: 'javascript:alert(1)', turnstileSiteKey: 7, assetBase: '' },
    } as unknown as { PIECEOFF_WEB_CONFIG?: unknown });
    expect(c).toEqual(DEFAULT_CONFIG);
    expect(readConfig({ PIECEOFF_WEB_CONFIG: 'nope' } as unknown as { PIECEOFF_WEB_CONFIG?: unknown })).toEqual(DEFAULT_CONFIG);
    // Plain http only for local development.
    expect(readConfig({ PIECEOFF_WEB_CONFIG: { api: 'http://evil.example' } }).api).toBe(DEFAULT_CONFIG.api);
    // The invite origin is an origin: no path, query or script scheme.
    for (const site of ['javascript:alert(1)', 'http://evil.example', 'https://x.example/join', 'https://x.example/?a=1']) {
      expect(readConfig({ PIECEOFF_WEB_CONFIG: { site } }).site).toBe(DEFAULT_CONFIG.site);
    }
    expect(readConfig({ PIECEOFF_WEB_CONFIG: { site: 'https://staging.pieceoff.app' } }).site).toBe('https://staging.pieceoff.app');
  });

  it('ws_url_follows_the_api_scheme', () => {
    expect(wsUrl('https://rooms.pieceoff.app', 'ABCD2345')).toBe('wss://rooms.pieceoff.app/ws?code=ABCD2345');
    expect(wsUrl('http://127.0.0.1:8787', 'ABCD2345')).toBe('ws://127.0.0.1:8787/ws?code=ABCD2345');
  });
});
