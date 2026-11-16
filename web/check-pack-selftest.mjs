// Negative test for check-pack.mjs's local-path leak check: tamper a copy of
// the tarball (a /home/ path appended to the wasm, a stray sourcemap) and
// require the check to fail with the leak message. Run by scripts/web-check.sh.
import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const tarball = process.argv[2];
if (!tarball) {
  console.error('usage: node check-pack-selftest.mjs <tarball>');
  process.exit(2);
}
const work = mkdtempSync(join(tmpdir(), 'pieceoff-pack-selftest.'));
try {
  const dir = join(work, 'x');
  execFileSync('mkdir', [dir]);
  execFileSync('tar', ['-xzf', tarball, '-C', dir]);
  appendFileSync(join(dir, 'pieceoff_web_wasm_bg.wasm'), '/home/someone/.cargo/registry/src/x.rs');
  writeFileSync(join(dir, 'play.js.map'), '{}');
  const bad = join(work, 'pieceoff-web-bad.tar.gz');
  execFileSync('sh', ['-c', `cd "${dir}" && tar -czf "${bad}" *`]);
  const r = spawnSync('node', [join(HERE, 'check-pack.mjs'), bad], { encoding: 'utf8', env: { ...process.env, PACK_CHECK_SKIP_REBUILD: '1' } });
  const out = r.stdout + r.stderr;
  if (r.status === 0 || !out.includes('leaks a local path') || !out.includes('unexpected file in tarball: play.js.map')) {
    console.error('selftest FAILED: the tampered tarball was not rejected as expected\n' + out);
    process.exit(1);
  }
  console.log('pack check selftest ok: tampered tarball rejected (local path leak, stray sourcemap).');
} finally {
  rmSync(work, { recursive: true, force: true });
}
