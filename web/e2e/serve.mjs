// Static server for the two-client e2e (scripts/web-e2e.sh): the release build
// (web/dist, web/public/builtin) through web/dev/serve.mjs's file handler, plus
// a bare page shell at `/` and `/join` (the site's invite path). The shell
// carries no configuration: the spec sets `window.PIECEOFF_WEB_CONFIG` with an
// init script, so a reload or an invite link opened in the same browser context
// gets the same config. Loopback only, on a free port; writes {"url"} as JSON
// to the file named by the first argument, then runs until SIGTERM/SIGINT.
import { createServer } from 'node:http';
import { writeFile } from 'node:fs/promises';
import { serveFile } from '../dev/serve.mjs';

const out = process.argv[2];
if (!out) {
  console.error('usage: node e2e/serve.mjs <details.json>');
  process.exit(2);
}

const SHELL = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
    <title>PieceOff (e2e)</title>
    <link rel="icon" href="data:," />
    <link rel="stylesheet" href="/play-app/play.css" />
    <script type="module" src="/play-app/play.js"></script>
  </head>
  <body>
    <div id="pieceoff-play"></div>
  </body>
</html>
`;

const server = createServer((req, res) => {
  const url = (req.url ?? '/').split('?')[0];
  if (url === '/' || url === '/join') {
    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
    return res.end(SHELL);
  }
  if (serveFile(req, res, url)) return;
  res.writeHead(404);
  res.end('not found');
});

server.listen(0, '127.0.0.1', async () => {
  const { port } = server.address();
  await writeFile(out, JSON.stringify({ url: `http://127.0.0.1:${port}` }));
  console.log(`e2e page server on http://127.0.0.1:${port}`);
});
const stop = () => server.close(() => process.exit(0));
process.on('SIGTERM', stop);
process.on('SIGINT', stop);
