// Bundles the renderer spike into dev/dist (gitignored) next to the core wasm.
import { build } from 'esbuild';
import { copyFileSync, mkdirSync } from 'node:fs';

const here = new URL('.', import.meta.url);
mkdirSync(new URL('dist/', here), { recursive: true });
await build({
  entryPoints: [new URL('spike.ts', here).pathname],
  outfile: new URL('dist/spike.js', here).pathname,
  bundle: true,
  format: 'esm',
  target: 'es2022',
  minify: true,
  sourcemap: true,
  legalComments: 'none',
  logLevel: 'info',
});
copyFileSync(
  new URL('../wasm/pieceoff_web_wasm_bg.wasm', here),
  new URL('dist/pieceoff_web_wasm_bg.wasm', here),
);
