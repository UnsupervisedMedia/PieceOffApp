// Browser room creation from the site (web/src/app/create.ts): the preflight
// for `x-turnstile-token`, and the site's CORS header on every `POST
// /api/rooms` answer (success and refusals) so the web client can read them.
// Only the site's own origin, never `*`. Also proves the exact create_room
// body the web client sends (a built-in loop, listed) is accepted, and the
// creator's resume token joins as host.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { connect, freePort, ofType, startServer } from "./helpers.mjs";

const SITE = "https://site.example";
const OTHER = "https://other.example";
const ACAO = "access-control-allow-origin";
const TIDAL = "06add36908c1cb4f50128d0c9133b38ceea20f873e2d0e1fae06e08795d07da5";

const cleanups = [];
after(async () => {
  for (const cleanup of cleanups.reverse()) await cleanup();
});

async function boot(vars = {}) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-createcors-"));
  const port = await freePort();
  const server = await startServer(port, dir, { SITE_ORIGIN: SITE, ALLOWED_ORIGINS: `${SITE},${OTHER}`, ...vars });
  cleanups.push(async () => {
    await server.stop();
    await rm(dir, { recursive: true, force: true });
  });
  return port;
}

// What web/src/app/create.ts sends for Tidal Glass, 48 pieces, listed.
const webBody = (over = {}) =>
  JSON.stringify({
    type: "create_room",
    name: "Web Host",
    title: "Tidal Glass",
    media_sha256: TIDAL,
    media_extension: "webp",
    width: 480,
    height: 320,
    rows: 6,
    columns: 8,
    initial_snapshot: null,
    duration_ms: 5999,
    player_key: "0123456789abcdef0123456789abcdef",
    cut_style: "jigsaw",
    listed: true,
    ...over,
  });

const post = (port, origin, body = webBody()) =>
  fetch(`http://127.0.0.1:${port}/api/rooms`, {
    method: "POST",
    headers: { origin, "content-type": "application/json", "x-turnstile-token": "dev" },
    body,
  });

describe("POST /api/rooms CORS for the web client", () => {
  it("preflight allows the site with the turnstile header", async () => {
    const port = await boot();
    const pre = await fetch(`http://127.0.0.1:${port}/api/rooms`, {
      method: "OPTIONS",
      headers: { origin: SITE, "access-control-request-method": "POST", "access-control-request-headers": "x-turnstile-token, content-type" },
    });
    assert.equal(pre.status, 204);
    assert.equal(pre.headers.get(ACAO), SITE);
    assert.equal(pre.headers.get("access-control-allow-methods"), "POST");
    assert.equal(pre.headers.get("access-control-allow-headers"), "x-turnstile-token, content-type");
    const foreign = await fetch(`http://127.0.0.1:${port}/api/rooms`, { method: "OPTIONS", headers: { origin: OTHER } });
    assert.equal(foreign.headers.get(ACAO), null);
  });

  it("the web client's body creates a listed built-in room the creator joins as host", async () => {
    const port = await boot();
    const made = await post(port, SITE);
    assert.equal(made.status, 200);
    assert.equal(made.headers.get(ACAO), SITE);
    const { code, client_id, resume_token } = await made.json();
    assert.match(code, /^[A-HJ-NP-Z2-9]{8}$/);

    const ws = await connect(port, code);
    ws.send({
      type: "join_room",
      invite_code: code,
      name: "Web Host",
      media_sha256: "",
      media_extension: "",
      spectator: false,
      player_key: "0123456789abcdef0123456789abcdef",
      resume_token,
    });
    const welcome = await ws.next(ofType("welcome"), "welcome");
    assert.equal(welcome.client_id, client_id);
    assert.equal(welcome.snapshot.host_id, client_id);
    assert.equal(welcome.snapshot.listed, true);
    ws.close();

    // A foreign (allowed) origin still creates, but cannot read the answer.
    const other = await post(port, OTHER);
    assert.equal(other.status, 200);
    assert.equal(other.headers.get(ACAO), null);
  });

  it("refusals carry the site header too (400 and 429)", async () => {
    const port = await boot();
    const bad = await post(port, SITE, webBody({ rows: 30, columns: 30 }));
    assert.equal(bad.status, 400);
    assert.equal(bad.headers.get(ACAO), SITE);
    assert.match((await bad.json()).error, /400 pieces/);
    let limited = null;
    for (let i = 0; i < 20 && limited === null; i++) {
      const r = await post(port, SITE);
      if (r.status === 429) limited = r;
    }
    assert.ok(limited, "the create budget ran out");
    assert.equal(limited.headers.get(ACAO), SITE);
  });

  it("paused: 503 readable by the site; lobby too", async () => {
    const port = await boot({ ONLINE_PLAY_ENABLED: 0 });
    const r = await post(port, SITE);
    assert.equal(r.status, 503);
    assert.equal(r.headers.get(ACAO), SITE);
    assert.match((await r.json()).error, /paused/);
    const lobby = await fetch(`http://127.0.0.1:${port}/api/lobby`, { headers: { origin: SITE } });
    assert.equal(lobby.headers.get(ACAO), SITE);
    const foreign = await post(port, OTHER);
    assert.equal(foreign.headers.get(ACAO), null);
  });
});
