import { afterEach, describe, expect, it, vi } from 'vitest';
import { TURNSTILE_SRC, loadTurnstile, renderTurnstile, resetTurnstileLoader, type TurnstileApi } from './turnstile';

afterEach(() => {
  resetTurnstileLoader();
  document.head.replaceChildren();
  delete (window as unknown as { turnstile?: unknown }).turnstile;
});

describe('turnstile', () => {
  it('loads_the_script_once_and_only_when_asked', async () => {
    expect(document.querySelectorAll('script')).toHaveLength(0);
    const a = loadTurnstile();
    const b = loadTurnstile();
    const scripts = document.querySelectorAll('script');
    expect(scripts).toHaveLength(1);
    expect(scripts[0]!.src).toBe(TURNSTILE_SRC);
    expect(TURNSTILE_SRC).toContain('render=explicit');
    const api = { render: vi.fn(() => 'w1'), reset: vi.fn(), remove: vi.fn() };
    (window as unknown as { turnstile: TurnstileApi }).turnstile = api;
    scripts[0]!.dispatchEvent(new Event('load'));
    expect(await a).toBe(api);
    expect(await b).toBe(api);
  });

  it('a_failed_load_can_be_retried', async () => {
    const a = loadTurnstile();
    document.querySelector('script')!.dispatchEvent(new Event('error'));
    await expect(a).rejects.toThrow();
    expect(document.querySelectorAll('script')).toHaveLength(0);
    void loadTurnstile().catch(() => undefined);
    expect(document.querySelectorAll('script')).toHaveLength(1);
  });

  it('renders_with_action_create_room', () => {
    const api = { render: vi.fn((_el: HTMLElement, _o: Record<string, unknown>) => 'w1'), reset: vi.fn(), remove: vi.fn() };
    const onToken = vi.fn();
    const w = renderTurnstile(api, document.createElement('div'), {
      siteKey: 'key-1',
      onToken,
      onExpire: vi.fn(),
      onError: vi.fn(),
    });
    const opts = api.render.mock.calls[0]![1];
    expect(opts).toMatchObject({ sitekey: 'key-1', action: 'create_room' });
    (opts.callback as (t: string) => void)('tok');
    expect(onToken).toHaveBeenCalledWith('tok');
    w.reset();
    expect(api.reset).toHaveBeenCalledWith('w1');
  });
});
