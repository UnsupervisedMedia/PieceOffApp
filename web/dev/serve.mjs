// Plain-HTTP static server for the renderer spike, reachable on the LAN so a
// phone can open it (WebGL and muted video need no secure context).
//   /dev/...      web/dev (spike.html, dist/)
//   /builtin/...  web/public/builtin (mp4 + posters, with Range support: iOS
//                 Safari will not play an mp4 served without byte ranges)
//   /play-app/... the lobby app build (web/dist, `npm run build`), with
//                 /play-app/builtin/ from web/public/builtin; open /dev/play.html
import { createServer } from 'node:http';
import { createReadStream, statSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { extname, normalize, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const PORT = Number(process.env.PORT ?? 8099);
const WEB = fileURLToPath(new URL('..', import.meta.url));
// Longest prefix first.
const ROOTS = {
  '/play-app/builtin/': join(WEB, 'public', 'builtin'),
  '/play-app/': join(WEB, 'dist'),
  '/dev/': join(WEB, 'dev'),
  '/builtin/': join(WEB, 'public', 'builtin'),
};
const TYPES = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.map': 'application/json',
  '.wasm': 'application/wasm', '.css': 'text/css; charset=utf-8', '.mp4': 'video/mp4', '.jpg': 'image/jpeg', '.json': 'application/json',
};

function resolve(url) {
  for (const [prefix, dir] of Object.entries(ROOTS)) {
    if (url.startsWith(prefix)) {
      let rel;
      try {
        rel = normalize(decodeURIComponent(url.slice(prefix.length))).replace(/^(\.\.[/\\])+/, '');
      } catch {
        return null; // malformed percent-escape: 404, not a crash
      }
      const path = join(dir, rel);
      return path.startsWith(dir) ? path : null;
    }
  }
  return null;
}

createServer((req, res) => {
  const url = (req.url ?? '/').split('?')[0];
  if (url === '/' || url === '/dev' || url === '/dev/') {
    res.writeHead(302, { Location: '/dev/spike.html' + ((req.url ?? '').includes('?') ? '?' + req.url.split('?')[1] : '') });
    return res.end();
  }
  const path = resolve(url);
  let st;
  try {
    st = path && statSync(path);
  } catch {}
  if (!st || !st.isFile()) {
    res.writeHead(404);
    return res.end('not found');
  }
  const headers = {
    'Content-Type': TYPES[extname(path)] ?? 'application/octet-stream',
    'Cache-Control': 'no-store',
    'Accept-Ranges': 'bytes',
  };
  const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? '');
  if (range) {
    let start = range[1] === '' ? st.size - Number(range[2]) : Number(range[1]);
    let end = range[1] !== '' && range[2] !== '' ? Number(range[2]) : st.size - 1;
    end = Math.min(end, st.size - 1);
    if (start < 0 || start > end) {
      res.writeHead(416, { 'Content-Range': `bytes */${st.size}` });
      return res.end();
    }
    res.writeHead(206, { ...headers, 'Content-Range': `bytes ${start}-${end}/${st.size}`, 'Content-Length': end - start + 1 });
    if (req.method === 'HEAD') return res.end();
    return createReadStream(path, { start, end }).pipe(res);
  }
  res.writeHead(200, { ...headers, 'Content-Length': st.size });
  if (req.method === 'HEAD') return res.end();
  createReadStream(path).pipe(res);
}).listen(PORT, '0.0.0.0', () => {
  const q = '?pieces=400';
  console.log(`spike: http://127.0.0.1:${PORT}/dev/spike.html${q}`);
  console.log(`lobby: http://127.0.0.1:${PORT}/dev/play.html (?api=http://127.0.0.1:8787 for a local Worker)`);
  for (const list of Object.values(networkInterfaces())) {
    for (const a of list ?? []) {
      if (a.family === 'IPv4' && !a.internal) console.log(`spike: http://${a.address}:${PORT}/dev/spike.html${q}`);
    }
  }
});
