import { describe, expect, it } from 'vitest';
import { VERSION_PLACEHOLDER } from './index';

describe('smoke', () => {
  it('exports the placeholder version', () => {
    expect(VERSION_PLACEHOLDER).toBe('web-0');
  });
});
