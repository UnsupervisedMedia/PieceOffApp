import { build } from 'esbuild';
import { copyFileSync } from 'node:fs';

await build({
  entryPoints: ['src/index.ts'],
  outfile: 'dist/play.js',
  bundle: true,
  format: 'esm',
  target: 'es2022',
  minify: true,
  sourcemap: false,
  legalComments: 'none',
  logLevel: 'info',
});

// The core wasm ships next to play.js; the app loads it from `assetBase`.
copyFileSync('wasm/pieceoff_web_wasm_bg.wasm', 'dist/pieceoff_web_wasm_bg.wasm');
