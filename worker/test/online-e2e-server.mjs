// Runs a local `wrangler dev` configured like production PieceOff Online
// (uploads off, Turnstile required, site origin https://pieceoff.app) but
// with a loopback Turnstile stand-in, for the desktop app's online e2e test
// (scripts/online-e2e.sh). Writes the connection details as JSON to the
// file named by the first argument, then runs until SIGTERM or SIGINT.
// The browser e2e (scripts/web-e2e.sh) sets PIECEOFF_E2E_SITE_ORIGIN to its
// local page's origin, which then is the site origin (CORS, socket origin
// check) instead of https://pieceoff.app; Turnstile stays required.
// Local only: nothing is deployed.
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fakeTurnstile, freePort, startServer } from "./helpers.mjs";

const out = process.argv[2];
if (!out) {
  console.error("usage: node test/online-e2e-server.mjs <details.json>");
  process.exit(2);
}
const SITE_ORIGIN = process.env.PIECEOFF_E2E_SITE_ORIGIN || "https://pieceoff.app";
const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-online-e2e-"));
const verifier = await fakeTurnstile();
const port = await freePort();
let server;
let stopping = false;
async function stop(code) {
  if (stopping) return;
  stopping = true;
  await server?.stop();
  await verifier.close();
  await rm(dir, { recursive: true, force: true });
  process.exit(code);
}
process.on("SIGTERM", () => stop(0));
process.on("SIGINT", () => stop(0));
try {
  server = await startServer(port, dir, {
    // Production settings (vars and defaults), except the Turnstile endpoint.
    FLUSH_MS: 5000,
    LOBBY_CACHE_SECONDS: 5,
    MEDIA_UPLOADS_ENABLED: 0,
    DEV_ALLOW_UNVERIFIED_CREATE: 0,
    TURNSTILE_SECRET: "e2e-secret",
    TURNSTILE_HOSTNAME: "pieceoff.app",
    TURNSTILE_VERIFY_URL: verifier.url,
    SITE_ORIGIN,
    ...(SITE_ORIGIN !== "https://pieceoff.app" ? { ALLOWED_ORIGINS: SITE_ORIGIN } : {}),
  });
} catch (error) {
  console.error(String(error));
  await stop(1);
}
await writeFile(
  out,
  JSON.stringify({
    url: `ws://127.0.0.1:${port}/ws`,
    api: `http://127.0.0.1:${port}`,
    site_origin: SITE_ORIGIN,
    turnstile_token: "good-link-device",
  }),
);
console.log(`PieceOff rooms Worker on ws://127.0.0.1:${port}/ws`);
setInterval(() => {}, 1 << 30);
