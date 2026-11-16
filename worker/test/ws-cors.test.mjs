// The /ws refusals answered before the Durable Object (503 paused, 429 lookup
// budget, 403 origin) carry the site's CORS header for the site's own origin
// only, so the web client can read why its upgrade failed. Never `*`.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { freePort, startServer } from "./helpers.mjs";

const SITE = "https://site.example";
const OTHER = "https://other.example";
const ACAO = "access-control-allow-origin";

const cleanups = [];
after(async () => {
  for (const cleanup of cleanups.reverse()) await cleanup();
});

async function boot(vars) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-wscors-"));
  const port = await freePort();
  const server = await startServer(port, dir, { SITE_ORIGIN: SITE, ...vars });
  cleanups.push(async () => {
    await server.stop();
    await rm(dir, { recursive: true, force: true });
  });
  return port;
}

const lookup = (port, origin, code = "ABCDEFGH") =>
  fetch(`http://127.0.0.1:${port}/ws?code=${code}`, { headers: { origin } });

describe("/ws refusal CORS", () => {
  it("503 paused: header for the site, none for a foreign origin", async () => {
    const port = await boot({ ONLINE_PLAY_ENABLED: 0 });
    const site = await lookup(port, SITE);
    assert.equal(site.status, 503);
    assert.equal(site.headers.get(ACAO), SITE);
    const foreign = await lookup(port, OTHER);
    assert.equal(foreign.status, 503);
    assert.equal(foreign.headers.get(ACAO), null);
    // Other paused routes are unchanged.
    const api = await fetch(`http://127.0.0.1:${port}/api/rooms`, { method: "POST", body: "{}", headers: { origin: SITE } });
    assert.equal(api.status, 503);
    assert.equal(api.headers.get(ACAO), null);
  });

  it("403 origin: header for the site, none for a foreign origin", async () => {
    // The site itself is not on the allowlist here, so it is refused too.
    const port = await boot({ ALLOWED_ORIGINS: "https://listed.example" });
    const site = await lookup(port, SITE);
    assert.equal(site.status, 403);
    assert.equal(site.headers.get(ACAO), SITE);
    const foreign = await lookup(port, "https://evil.example");
    assert.equal(foreign.status, 403);
    assert.equal(foreign.headers.get(ACAO), null);
  });

  it("429 lookup budget: header for the site, none for a foreign origin", async () => {
    const port = await boot({ ALLOWED_ORIGINS: `${SITE},${OTHER}` });
    let site = null;
    for (let i = 0; i < 40 && site === null; i++) {
      const response = await lookup(port, SITE);
      if (response.status === 429) site = response;
    }
    assert.ok(site, "the lookup budget ran out");
    assert.equal(site.headers.get(ACAO), SITE);
    assert.equal(site.headers.get("vary"), "Origin");
    const foreign = await lookup(port, OTHER);
    assert.equal(foreign.status, 429);
    assert.equal(foreign.headers.get(ACAO), null);
  });
});
