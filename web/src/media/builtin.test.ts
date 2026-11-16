// @vitest-environment node
import { existsSync, readFileSync, statSync } from 'node:fs';
import { execFileSync, spawnSync } from 'node:child_process';
import { beforeAll, describe, expect, it } from 'vitest';
import { builtin_media_json } from '../../wasm/pieceoff_web_wasm.js';
import { initWasm } from '../test-wasm';
import { builtinAssets } from './builtin';

const DIR = new URL('../../public/builtin/', import.meta.url);
const manifest: Record<string, { mp4: string; poster: string }> = JSON.parse(
  readFileSync(new URL('manifest.json', DIR), 'utf8'),
);
let core: { sha256: string; title: string }[];

beforeAll(async () => {
  await initWasm();
  core = JSON.parse(builtin_media_json());
});

const hasFfprobe = spawnSync('ffprobe', ['-version']).status === 0;

describe('built-in media manifest', () => {
  it('manifest_keys_equal_core_builtin_list', () => {
    expect(core.length).toBe(8);
    expect(Object.keys(manifest).sort()).toEqual(core.map((m) => m.sha256).sort());
  });

  it('every_manifest_file_exists', () => {
    for (const [sha, e] of Object.entries(manifest)) {
      expect(e.mp4).toBe(`${sha}.mp4`);
      expect(e.poster).toBe(`${sha}.jpg`);
      for (const f of [e.mp4, e.poster]) {
        const u = new URL(f, DIR);
        expect(existsSync(u), f).toBe(true);
        expect(statSync(u).size, f).toBeGreaterThan(0);
      }
      expect(statSync(new URL(e.mp4, DIR)).size).toBeLessThan(600 * 1024);
    }
  });

  it('posters are valid JPEGs', () => {
    for (const e of Object.values(manifest)) {
      const b = readFileSync(new URL(e.poster, DIR));
      expect([b[0], b[1], b[2]]).toEqual([0xff, 0xd8, 0xff]);
      expect([b[b.length - 2], b[b.length - 1]]).toEqual([0xff, 0xd9]);
    }
  });

  it.skipIf(!hasFfprobe)('mp4s are 3:2 video-only', () => {
    for (const e of Object.values(manifest)) {
      const out = execFileSync('ffprobe', [
        '-v', 'error', '-show_entries', 'stream=codec_type,codec_name,width,height,pix_fmt',
        '-of', 'json', new URL(e.mp4, DIR).pathname,
      ]).toString();
      const streams = JSON.parse(out).streams as Record<string, unknown>[];
      expect(streams.map((s) => s.codec_type)).toEqual(['video']);
      const v = streams[0] as { codec_name: string; width: number; height: number; pix_fmt: string };
      expect(v.codec_name).toBe('h264');
      expect(v.pix_fmt).toBe('yuv420p');
      expect(v.width * 2).toBe(v.height * 3);
    }
  });

  it.skipIf(!hasFfprobe)('mp4s play exactly the webp loop (72 frames, 6 s)', () => {
    for (const e of Object.values(manifest)) {
      const out = execFileSync('ffprobe', [
        '-v', 'error', '-count_frames', '-show_entries', 'stream=nb_read_frames,duration',
        '-of', 'csv=p=0', new URL(e.mp4, DIR).pathname,
      ]).toString().trim();
      expect(out).toBe('6.000000,72');
    }
  });

  it('unknown_sha_returns_null', () => {
    expect(builtinAssets('0'.repeat(64))).toBeNull();
    expect(builtinAssets('')).toBeNull();
  });

  it('uppercase_sha_resolves', () => {
    const { sha256, title } = core[0];
    const a = builtinAssets(sha256.toUpperCase());
    expect(a).toEqual({ mp4: `/builtin/${sha256}.mp4`, poster: `/builtin/${sha256}.jpg`, title });
    expect(builtinAssets(sha256, 'https://x.test/b/')?.mp4).toBe(`https://x.test/b/${sha256}.mp4`);
  });
});
