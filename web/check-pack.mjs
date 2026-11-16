// Verifies the web release tarball (web/pack.sh), run by scripts/web-check.sh:
//   node check-pack.mjs [path/to/pieceoff-web-<version>.tar.gz]
// It unpacks the tarball and checks: only allowed files are in it (no
// sourcemaps, dev/spike/e2e/test sources); every file play-app.json and
// builtin/manifest.json reference is present; the sha256 lists (play-app.json
// and SHA256SUMS) match the bytes; every relative import in the JS resolves
// inside the tarball; version/protocol match the repo; the initial JS + WASM +
// CSS gzip budget holds on the packed bytes; entries are normalised (owner 0,
// fixed mtime, sorted); and packing twice gives byte-identical output.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const HERE = dirname(fileURLToPath(import.meta.url));
const BUDGET = 400 * 1024;
const MTIME = '2020-01-01';
const sha = (buf) => createHash('sha256').update(buf).digest('hex');
const problems = [];
const bad = (msg) => problems.push(msg);

const appVersion = /^version = "(.*)"$/m.exec(readFileSync(join(HERE, '../crates/pieceoff-app/Cargo.toml'), 'utf8'))[1];
const tarball = resolve(process.argv[2] ?? join(HERE, '../dist', `pieceoff-web-${appVersion}.tar.gz`));
let tarBytes;
try {
  tarBytes = readFileSync(tarball);
} catch {
  console.error(`pack check: ${tarball} does not exist (run web/pack.sh)`);
  process.exit(1);
}

const work = mkdtempSync(join(tmpdir(), 'pieceoff-pack-check.'));
const cleanup = () => rmSync(work, { recursive: true, force: true });
process.on('exit', cleanup);
const out = join(work, 'unpacked');
execFileSync('mkdir', [out]);
execFileSync('tar', ['-xzf', tarball, '-C', out]);

// Walk the unpacked tree.
const files = new Map(); // posix relative path -> Buffer
(function walk(dir, rel) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const r = rel ? `${rel}/${name}` : name;
    if (statSync(p).isDirectory()) walk(p, r);
    else files.set(r, readFileSync(p));
  }
})(out, '');

// 1. Allow-list: nothing else ships.
const FIXED = new Set(['play-app.json', 'SHA256SUMS', 'LICENSE', 'THIRD_PARTY_NOTICES.txt']);
const allowed = (p) =>
  FIXED.has(p) ||
  p === 'play.js' || p === 'play.css' || p === 'pieceoff_web_wasm_bg.wasm' ||
  /^chunks\/[\w-]+\.js$/.test(p) ||
  /^builtin\/([0-9a-f]{64}\.(mp4|jpg)|manifest\.json)$/.test(p);
for (const p of files.keys()) if (!allowed(p)) bad(`unexpected file in tarball: ${p}`);
for (const [p, buf] of files) {
  if (/\.js$/.test(p) && /sourceMappingURL|\/\/# sourceURL/.test(buf.toString('latin1'))) bad(`${p}: sourcemap reference`);
  if (/\.js$/.test(p) && /node_modules|web\/dev\/|spike|e2e\//.test(buf.toString('latin1').replace(/\/\/ .*$/gm, ''))) {
    // Source paths leaking into the minified output (comments aside) mean a dev entry got bundled.
    bad(`${p}: references a dev/spike/e2e/node_modules path`);
  }
}

// 2. play-app.json and everything it references.
let idx = {};
try {
  idx = JSON.parse(files.get('play-app.json')?.toString('utf8') ?? '');
} catch {
  bad('play-app.json missing or not JSON');
}
const wasmPath = idx.wasm;
for (const k of ['version', 'protocol', 'entry', 'css', 'wasm', 'files']) if (idx[k] === undefined) bad(`play-app.json: missing "${k}"`);
if (idx.version !== appVersion) bad(`play-app.json version ${idx.version} != crates/pieceoff-app version ${appVersion}`);
if (!tarball.endsWith(`pieceoff-web-${appVersion}.tar.gz`)) bad(`tarball name does not carry version ${appVersion}`);
for (const k of ['entry', 'css', 'wasm']) {
  if (idx[k] && !files.has(idx[k])) bad(`play-app.json ${k} "${idx[k]}" is not in the tarball`);
  if (idx[k] && !idx.files?.[idx[k]]) bad(`play-app.json ${k} "${idx[k]}" is not in files`);
}
const listed = idx.files ?? {};
for (const [p, h] of Object.entries(listed)) {
  const buf = files.get(p);
  if (!buf) bad(`play-app.json lists ${p}, not in the tarball`);
  else if (sha(buf) !== h) bad(`play-app.json sha256 for ${p} does not match`);
}
for (const p of files.keys()) {
  if (p !== 'play-app.json' && p !== 'SHA256SUMS' && !(p in listed)) bad(`${p} is in the tarball but not in play-app.json files`);
}
// Protocol: the wasm's own answer, and the generated TS types, must agree with the index.
try {
  const glue = await import(join(HERE, 'wasm/pieceoff_web_wasm.js'));
  glue.initSync({ module: files.get(wasmPath ?? 'pieceoff_web_wasm_bg.wasm') });
  if (glue.protocol_version() !== idx.protocol) bad(`play-app.json protocol ${idx.protocol} != packed wasm protocol_version() ${glue.protocol_version()}`);
  const gen = /PROTOCOL_VERSION = (\d+)/.exec(readFileSync(join(HERE, 'src/protocol.gen.ts'), 'utf8'));
  if (!gen || Number(gen[1]) !== idx.protocol) bad(`protocol.gen.ts PROTOCOL_VERSION ${gen?.[1]} != ${idx.protocol}`);
} catch (e) {
  bad(`cannot load the packed wasm to read its protocol version: ${e.message}`);
}

// 3. SHA256SUMS covers every other file, exactly.
const sums = new Map();
for (const line of (files.get('SHA256SUMS')?.toString('utf8') ?? '').split('\n').filter(Boolean)) {
  const m = /^([0-9a-f]{64}) {2}(.+)$/.exec(line);
  if (!m) bad(`SHA256SUMS: malformed line "${line}"`);
  else sums.set(m[2], m[1]);
}
for (const [p, buf] of files) {
  if (p === 'SHA256SUMS') continue;
  if (!sums.has(p)) bad(`SHA256SUMS lacks ${p}`);
  else if (sums.get(p) !== sha(buf)) bad(`SHA256SUMS hash for ${p} does not match`);
}
for (const p of sums.keys()) if (!files.has(p)) bad(`SHA256SUMS lists ${p}, not in the tarball`);

// 4. builtin/manifest.json: every mp4 and poster is there.
try {
  const manifest = JSON.parse(files.get('builtin/manifest.json').toString('utf8'));
  const ids = Object.keys(manifest);
  if (!ids.length) bad('builtin/manifest.json is empty');
  for (const [id, e] of Object.entries(manifest)) {
    for (const k of ['mp4', 'poster']) {
      if (!e[k] || !files.has(`builtin/${e[k]}`)) bad(`builtin/manifest.json ${id}.${k} "${e[k]}" is not in the tarball`);
    }
  }
  for (const p of files.keys()) {
    const m = /^builtin\/(.{64})\.(mp4|jpg)$/.exec(p);
    if (m && !manifest[m[1]]) bad(`${p} is not referenced by builtin/manifest.json`);
  }
} catch {
  bad('builtin/manifest.json missing or not JSON');
}

// 5. Relative imports resolve inside the tarball; the static graph from the
//    entry is the lobby's initial load.
// esbuild's minified output: `from"./x.js"`, `import"./x.js"`, `import("./x.js")`.
const IMPORT = /(\bimport\s*\(\s*|\bfrom\s*|\bimport\s*)"(\.{1,2}\/[^"]+)"/g;
const importsOf = (file) => {
  const stat = [];
  const dyn = [];
  const src = files.get(file).toString('utf8');
  for (const m of src.matchAll(IMPORT)) {
    const target = posix.normalize(posix.join(posix.dirname(file), m[2]));
    (m[1].includes('(') ? dyn : stat).push(target);
  }
  return { stat, dyn };
};
const initial = new Set();
(function walk(f) {
  if (initial.has(f)) return;
  if (!files.has(f)) return bad(`import of ${f}, not in the tarball`);
  initial.add(f);
  const { stat, dyn } = importsOf(f);
  stat.forEach(walk);
  for (const d of dyn) if (!files.has(d)) bad(`${f}: dynamic import of ${d}, not in the tarball`);
})(idx.entry ?? 'play.js');
for (const f of files.keys()) if (/\.js$/.test(f)) for (const d of importsOf(f).dyn) if (!files.has(d)) bad(`${f}: dynamic import of ${d}, not in the tarball`);
const lazyJs = [...files.keys()].filter((f) => f.endsWith('.js') && !initial.has(f));
if (!lazyJs.length) bad('no lazy chunk: the board is not split out of the entry');

// 6. Size budget on the packed bytes.
const gz = (p) => gzipSync(files.get(p) ?? Buffer.alloc(0), { level: 9 }).length;
let total = gz(idx.wasm ?? '') + gz(idx.css ?? '');
for (const f of initial) total += gz(f);
const lazy = lazyJs.reduce((n, f) => n + gz(f), 0);
console.log(`packed initial (entry js + wasm + css) ${total} B gzip (budget ${BUDGET}); lazy board chunk(s) ${lazy} B gzip`);
if (total > BUDGET) bad(`initial gzip ${total} B exceeds the ${BUDGET} B budget`);

// 7. Entry metadata: sorted, owner 0, fixed mtime.
const verbose = execFileSync('tar', ['-tvzf', tarball], { encoding: 'utf8', env: { ...process.env, TZ: 'UTC' } }).split('\n').filter(Boolean);
const names = verbose.map((l) => l.split(/\s+/).slice(5).join(' '));
if (names.join('\n') !== [...names].sort().join('\n')) bad('tar entries are not sorted by name');
for (const l of verbose) {
  const cols = l.split(/\s+/);
  if (cols[1] !== '0/0') bad(`tar entry owner is ${cols[1]}, expected 0/0: ${l}`);
  if (cols[3] !== MTIME) bad(`tar entry mtime is ${cols[3]}, expected ${MTIME}: ${l}`);
}
if (tarBytes[0] !== 0x1f || tarBytes[1] !== 0x8b) bad('not a gzip file');
else if (tarBytes.readUInt32LE(4) !== 0) bad('gzip header carries an mtime (use gzip -n)');
else if (tarBytes[3] !== 0) bad('gzip header carries a file name (use gzip -n)');

// 8. Deterministic: pack again (rebuilding) into another directory; same bytes.
if (!process.env.PACK_CHECK_SKIP_REBUILD) {
  const again = join(work, 'again');
  execFileSync('sh', [join(HERE, 'pack.sh'), '--out', again], { stdio: ['ignore', 'ignore', 'inherit'] });
  const second = readFileSync(join(again, `pieceoff-web-${appVersion}.tar.gz`));
  const a = sha(tarBytes);
  const b = sha(second);
  console.log(`determinism: ${a}\n             ${b}`);
  if (a !== b) bad('packing twice gave different bytes');
}

if (problems.length) {
  console.error(`pack check FAILED (${problems.length}):\n  ` + problems.join('\n  '));
  process.exit(1);
}
console.log(`pack check ok: ${files.size} files, ${(tarBytes.length / 1024).toFixed(0)} KiB tarball.`);
