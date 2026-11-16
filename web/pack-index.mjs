// Writes play-app.json and SHA256SUMS for web/pack.sh into the staged tree:
//   node pack-index.mjs <stage-dir> <version>
// play-app.json is what a host page needs: the entry script, the stylesheet,
// the wasm, the protocol version the packed wasm reports, and a sha256 for
// every other file. SHA256SUMS (sha256sum -c format) covers everything but itself.
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const [stage, version] = process.argv.slice(2);
if (!stage || !version) {
  console.error('usage: node pack-index.mjs <stage-dir> <version>');
  process.exit(2);
}
const HERE = dirname(fileURLToPath(import.meta.url));
const sha = (buf) => createHash('sha256').update(buf).digest('hex');
const sorted = (a) => [...a].sort((x, y) => (x < y ? -1 : x > y ? 1 : 0));

const list = (dir, rel = '') =>
  sorted(readdirSync(dir)).flatMap((n) => {
    const p = join(dir, n);
    const r = rel ? `${rel}/${n}` : n;
    return statSync(p).isDirectory() ? list(p, r) : [r];
  });

const glue = await import(join(HERE, 'wasm/pieceoff_web_wasm.js'));
glue.initSync({ module: readFileSync(join(stage, 'pieceoff_web_wasm_bg.wasm')) });

const files = {};
for (const f of list(stage)) files[f] = sha(readFileSync(join(stage, f)));
const index = {
  version,
  protocol: glue.protocol_version(),
  entry: 'play.js',
  css: 'play.css',
  wasm: 'pieceoff_web_wasm_bg.wasm',
  files,
};
writeFileSync(join(stage, 'play-app.json'), JSON.stringify(index, null, 2) + '\n');

writeFileSync(
  join(stage, 'SHA256SUMS'),
  list(stage)
    .map((f) => `${sha(readFileSync(join(stage, f)))}  ${f}\n`)
    .join(''),
);
