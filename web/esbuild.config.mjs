import { build } from 'esbuild';
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';

// dist/ is the release bundle: start clean so no stale chunk ships.
rmSync('dist', { recursive: true, force: true });
mkdirSync('dist', { recursive: true });

// Code splitting: play.js is the lobby entry; the board (WebGL, gestures,
// overlays) is a lazy chunk under chunks/ reached only through
// app/loadBoard.ts's dynamic import. Shared modules land in common chunks.
const result = await build({
  entryPoints: { play: 'src/index.ts' },
  outdir: 'dist',
  chunkNames: 'chunks/[name]-[hash]',
  splitting: true,
  bundle: true,
  format: 'esm',
  target: 'es2022',
  minify: true,
  sourcemap: false,
  legalComments: 'none',
  logLevel: 'info',
  metafile: true,
});

// The core wasm ships next to play.js; the app loads it from `assetBase`.
copyFileSync('wasm/pieceoff_web_wasm_bg.wasm', 'dist/pieceoff_web_wasm_bg.wasm');

// For scripts/web-check.sh: which outputs the entry pulls in up front.
mkdirSync('build', { recursive: true });
writeFileSync('build/meta.json', JSON.stringify(result.metafile));
