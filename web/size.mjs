// Bundle size report and budget (run by scripts/web-check.sh after a build).
// "Initial" is what the lobby needs before anything lazy: play.js, every chunk
// it imports statically, the core wasm and play.css, gzipped (-9). The spec's
// budget is initial JS + WASM <= 400 KB gzip; the board chunk loads lazily
// and is reported, not budgeted.
import { readFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';

const BUDGET = 400 * 1024;
const meta = JSON.parse(readFileSync('build/meta.json', 'utf8'));
const gz = (path) => gzipSync(readFileSync(path), { level: 9 }).length;
const outputs = meta.outputs;

const entry = 'dist/play.js';
const initial = new Set();
const walk = (file) => {
  if (initial.has(file)) return;
  initial.add(file);
  for (const imp of outputs[file]?.imports ?? []) {
    if (imp.kind === 'import-statement' && !imp.external) walk(imp.path);
  }
};
walk(entry);
const lazy = Object.keys(outputs).filter((f) => f.endsWith('.js') && !initial.has(f));

const row = (label, bytes) => console.log(`${label.padEnd(44)} ${String(bytes).padStart(8)} B gzip`);
let js = 0;
for (const f of initial) {
  const b = gz(f);
  js += b;
  row(`entry js  ${f}`, b);
}
const wasm = gz('dist/pieceoff_web_wasm_bg.wasm');
const css = gz('dist/play.css');
row('wasm      dist/pieceoff_web_wasm_bg.wasm', wasm);
row('css       dist/play.css', css);
let lazyTotal = 0;
for (const f of lazy) {
  const b = gz(f);
  lazyTotal += b;
  row(`lazy      ${f}`, b);
}
const total = js + wasm + css;
row('initial (entry js + wasm + css)', total);
row('lazy total (board chunk)', lazyTotal);
console.log(`budget ${BUDGET} B: ${total <= BUDGET ? 'ok' : 'OVER'}`);
if (!lazy.length) {
  console.error('no lazy chunk: the board is bundled into the entry (code splitting broke)');
  process.exit(1);
}
if (total > BUDGET) {
  console.error('initial JS + WASM + CSS exceeds the 400 KB gzip budget');
  process.exit(1);
}
