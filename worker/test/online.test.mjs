// PieceOff Online: service info, device links, device-key rooms, the global
// media budget, the lobby, and the uploads-off launch setting, against a
// local `wrangler dev`.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { connect, delay, fakeTurnstile, freePort, joinMessage, ofType, rawRequest, startServer } from "./helpers.mjs";

const SITE = "https://pieceoff.app";
const SLOW_ORBIT = "f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80";
const TIDAL_GLASS = "06add36908c1cb4f50128d0c9133b38ceea20f873e2d0e1fae06e08795d07da5";
const UPLOADS_OFF = "PieceOff Online uses built-in puzzles for now. Use Local or LAN to play your own files.";
const MEDIA_FULL = "PieceOff Online is out of media space right now. Try a smaller file or try later.";

const cleanups = [];
after(async () => {
  for (const cleanup of cleanups.reverse()) await cleanup();
});

async function boot(vars) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-online-"));
  const port = await freePort();
  const server = await startServer(port, dir, vars);
  cleanups.push(async () => {
    await server.stop();
    await rm(dir, { recursive: true, force: true });
  });
  return port;
}

/** Stand-in for Turnstile siteverify (see `fakeTurnstile` in helpers). */
async function fakeVerifier() {
  const verifier = await fakeTurnstile();
  cleanups.push(verifier.close);
  return verifier.url;
}

const roomBody = (over = {}) => JSON.stringify({
  type: "create_room", name: "Host", title: "Anything the host typed", media_sha256: SLOW_ORBIT,
  media_extension: "webp", width: 640, height: 360, rows: 3, columns: 4, duration_ms: 5000, ...over,
});

const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function waitFor(read, predicate, label, ms = 15_000, every = 700) {
  let last;
  for (const end = Date.now() + ms; Date.now() < end; await delay(every)) {
    last = await read();
    if (predicate(last)) return last;
  }
  assert.fail(`timed out waiting for ${label}; last: ${JSON.stringify(last)}`);
}

describe("device links, device-key rooms and the lobby", () => {
  let port;
  const call = (method, url, headers = {}, body = null) => rawRequest(port, method, url, headers, body);
  const lobby = async () => JSON.parse((await call("GET", "/api/lobby")).text).rooms;

  before(async () => {
    port = await boot({
      DEV_ALLOW_UNVERIFIED_CREATE: 0,
      TURNSTILE_SECRET: "secret",
      TURNSTILE_HOSTNAME: "pieceoff.app",
      TURNSTILE_VERIFY_URL: await fakeVerifier(),
      SITE_ORIGIN: SITE,
      ADMIN_TOKEN: "admin-secret",
      DEVICE_ROOMS_PER_DAY: 3,
      LOBBY_THROTTLE_MS: 500,
      LOBBY_HEARTBEAT_MS: 1000,
    });
  });

  async function link(deviceName = "Test PC") {
    const made = await call("POST", "/api/links", { "content-type": "application/json" }, JSON.stringify({ device_name: deviceName }));
    assert.equal(made.status, 200, made.text);
    const body = JSON.parse(made.text);
    const approved = await call("POST", `/api/links/${body.link_code}/approve`, { origin: SITE, "x-turnstile-token": "good-link-device" });
    assert.equal(approved.status, 200, approved.text);
    const polled = await call("GET", `/api/links/${body.link_code}`, { authorization: `Bearer ${body.poll_secret}` });
    assert.equal(polled.status, 200, polled.text);
    return JSON.parse(polled.text).device_key;
  }

  const createWith = (key, over = {}) =>
    call("POST", "/api/rooms", { "content-type": "application/json", authorization: `Bearer ${key}` }, roomBody(over));

  it("describes the hosted service", async () => {
    const response = await call("GET", "/api/info");
    assert.equal(response.status, 200);
    assert.equal(response.headers["cache-control"], "no-store");
    assert.deepEqual(JSON.parse(response.text), {
      service: "pieceoff-rooms", protocol: 15, hosted: true,
      link_url: "https://pieceoff.app/link", share_url: "https://pieceoff.app/join",
      features: ["device_keys", "lobby"], uploads: true,
    });
  });

  it("links a device: create, poll, approve from the site with Turnstile, collect the key once", async () => {
    const made = await call("POST", "/api/links", {}, "");
    assert.equal(made.status, 200);
    const body = JSON.parse(made.text);
    assert.match(body.link_code, /^[A-HJ-NP-Z2-9]{8}$/);
    assert.match(body.poll_secret, /^[A-Za-z0-9_-]{43}$/);
    assert.equal(body.url, `https://pieceoff.app/link?c=${body.link_code}`);
    assert.equal(body.expires_in, 600);
    const poll = (secret = body.poll_secret) =>
      call("GET", `/api/links/${body.link_code}`, { authorization: `Bearer ${secret}` });

    const pending = await poll();
    assert.equal(pending.status, 202);
    assert.deepEqual(JSON.parse(pending.text), { status: "pending" });
    assert.equal((await poll("x".repeat(43))).status, 404, "wrong poll secret");

    const approveUrl = `/api/links/${body.link_code}/approve`;
    const preflight = await call("OPTIONS", approveUrl, {
      origin: SITE, "access-control-request-method": "POST", "access-control-request-headers": "x-turnstile-token, content-type",
    });
    assert.equal(preflight.status, 204);
    assert.equal(preflight.headers["access-control-allow-origin"], SITE);
    assert.equal(preflight.headers["access-control-allow-methods"], "POST");
    assert.equal(preflight.headers["access-control-allow-headers"], "x-turnstile-token, content-type");

    assert.equal((await call("POST", approveUrl, { "x-turnstile-token": "good-link-device" })).status, 403, "no origin");
    assert.equal((await call("POST", approveUrl, { origin: "https://evil.example", "x-turnstile-token": "good-link-device" })).status, 403);
    const wrongAction = await call("POST", approveUrl, { origin: SITE, "x-turnstile-token": "good-create-room" });
    assert.equal(wrongAction.status, 403);
    assert.equal(wrongAction.headers["access-control-allow-origin"], SITE);
    const ok = await call("POST", approveUrl, { origin: SITE, "x-turnstile-token": "good-link-device" });
    assert.equal(ok.status, 200);
    assert.deepEqual(JSON.parse(ok.text), { ok: true });
    assert.equal(ok.headers["access-control-allow-origin"], SITE);
    assert.equal((await call("POST", approveUrl, { origin: SITE, "x-turnstile-token": "good-link-device" })).status, 409);
    assert.equal((await call("POST", "/api/links/ABCDEFGH/approve", { origin: SITE, "x-turnstile-token": "good-link-device" })).status, 404);

    const approved = await poll();
    assert.equal(approved.status, 200);
    const { status, device_key: key } = JSON.parse(approved.text);
    assert.equal(status, "approved");
    assert.match(key, /^pok_[A-Za-z0-9_-]{43}$/);
    assert.equal((await poll()).status, 404, "the key is handed out once");
  });

  it("creates rooms with a device key, caps them per day, and asks to relink for bad or revoked keys", async () => {
    const key = await link();
    for (let i = 0; i < 3; i++) {
      const made = await createWith(key);
      assert.equal(made.status, 200, made.text);
      const body = JSON.parse(made.text);
      assert.deepEqual(Object.keys(body).sort(), ["client_id", "code", "resume_token"]);
    }
    assert.equal((await createWith(key)).status, 429, "daily room cap");

    const unknown = await createWith(`pok_${"A".repeat(43)}`);
    assert.equal(unknown.status, 401);
    assert.equal(JSON.parse(unknown.text).relink, true);
    assert.equal(JSON.parse((await createWith("not-a-key")).text).relink, true);

    const revoke = (token, body) => call("POST", "/api/admin/devices/revoke",
      { authorization: `Bearer ${token}`, "content-type": "application/json" }, JSON.stringify(body));
    assert.equal((await revoke("wrong", { device_key: key })).status, 403);
    const other = await link("Other PC");
    const revoked = await revoke("admin-secret", { device_key: other });
    assert.equal(revoked.status, 200, revoked.text);
    const refused = await createWith(other);
    assert.equal(refused.status, 401);
    assert.deepEqual(Object.keys(JSON.parse(refused.text)).sort(), ["error", "relink"]);
  });

  it("lists a built-in room publicly with its bundled title, updates, and removes it", async () => {
    const key = await link("Lobby PC");
    const refused = await createWith(key, { listed: true, media_sha256: "b".repeat(64), media_extension: "mp4" });
    assert.equal(refused.status, 400, "listed rooms take built-in media only");

    const made = await createWith(key, { listed: true, name: "Host‮ with a very long name indeed" });
    assert.equal(made.status, 200, made.text);
    const { code, resume_token: token } = JSON.parse(made.text);
    assert.equal((await lobby()).some((e) => e.code === code), false, "nobody playing yet");

    const host = await connect(port, code);
    host.send(joinMessage(code, "Host‮ with a very long name indeed", token, SLOW_ORBIT));
    await host.next(ofType("welcome"), "host welcome");
    const response = await call("GET", "/api/lobby");
    assert.equal(response.headers["access-control-allow-origin"], SITE);
    assert.equal(response.headers["cache-control"], "public, max-age=5");
    const [entry] = (await waitFor(lobby, (rooms) => rooms.some((e) => e.code === code), "listing"))
      .filter((e) => e.code === code);
    assert.equal(entry.title, "Slow Orbit");
    assert.ok(entry.host_name.length <= 24 && !/[‮\u0000-\u001F]/.test(entry.host_name), entry.host_name);
    assert.deepEqual(
      { players: entry.players, max_players: entry.max_players, spectators: entry.spectators, pieces: entry.pieces, placed: entry.placed, cut_style: entry.cut_style },
      { players: 1, max_players: 8, spectators: 0, pieces: 12, placed: 0, cut_style: "jigsaw" },
    );
    assert.ok(entry.created_at > 0 && entry.updated_at >= entry.created_at);

    host.send({ type: "queue_add", sha256: "c".repeat(64), extension: "mp4", title: "Mine", width: 640, height: 360, duration_ms: 1000 });
    assert.match((await host.next(ofType("error"), "refused queue item")).message, /built-in/);

    await delay(600);
    const watcher = await connect(port, code);
    watcher.send({ ...joinMessage(code, "Watcher", undefined, SLOW_ORBIT), spectator: true });
    await watcher.next(ofType("welcome"), "spectator welcome");
    await waitFor(lobby, (rooms) => rooms.find((e) => e.code === code)?.spectators === 1, "spectator count");

    host.send({ type: "set_listed", listed: false });
    await waitFor(lobby, (rooms) => !rooms.some((e) => e.code === code), "delisting");
    await delay(600);
    host.send({ type: "set_listed", listed: true });
    await waitFor(lobby, (rooms) => rooms.some((e) => e.code === code), "relisting");

    host.close();
    await waitFor(lobby, (rooms) => !rooms.some((e) => e.code === code), "removal when the last player leaves");
    watcher.close();
  });
});

describe("global media budget and lobby expiry", () => {
  let port;
  before(async () => {
    port = await boot({
      MEDIA_GLOBAL_QUOTA_BYTES: 1500,
      BUDGET_RECONCILE_MS: 1000,
      FAIL_BUDGET_RELEASE_ONCE: 1,
      IDLE_CLEANUP_MS: 1500,
      LOBBY_EXPIRY_MS: 3000,
    });
  });

  async function hostedRoom() {
    const bytes = randomBytes(1000);
    const made = await rawRequest(port, "POST", "/api/rooms", { "content-type": "application/json" },
      roomBody({ media_sha256: sha(bytes), media_extension: "mp4" }));
    assert.equal(made.status, 200, made.text);
    const { code, resume_token: token } = JSON.parse(made.text);
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", token, sha(bytes)));
    const welcome = await host.next(ofType("welcome"), "host welcome");
    const start = () => fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media`, {
      method: "POST",
      headers: { authorization: `Bearer ${welcome.media_upload_token}`, "content-type": "application/json" },
      body: JSON.stringify({ sha256: sha(bytes), size: bytes.length, part_size: bytes.length }),
    });
    const abort = (uploadId) => fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media/${uploadId}`, {
      method: "DELETE", headers: { authorization: `Bearer ${welcome.media_upload_token}` },
    });
    return { host, start, abort };
  }

  it("refuses past the budget with 507, reconciles drift, and releases on abort and room expiry", async () => {
    const a = await hostedRoom();
    const b = await hostedRoom();
    const first = await a.start();
    assert.equal(first.status, 200);
    const { upload_id: aUpload } = await first.json();
    const full = await b.start();
    assert.equal(full.status, 507);
    assert.deepEqual(await full.json(), { error: MEDIA_FULL });

    // The abort's release is lost (test hook: as if the object died there);
    // the room's next reconcile corrects the registry.
    assert.equal((await a.abort(aUpload)).status, 200);
    assert.equal((await b.start()).status, 507, "drift until the room reconciles");
    const started = await waitFor(async () => {
      const response = await b.start();
      return { status: response.status, body: await response.json() };
    }, (r) => r.status === 200, "reconcile", 20_000, 1000);

    // A working abort frees the budget at once.
    assert.equal((await b.abort(started.body.upload_id)).status, 200);
    assert.equal((await a.start()).status, 200);
    assert.equal((await b.start()).status, 507);

    // Room A expires once empty; its media budget goes with it.
    a.host.close();
    await waitFor(async () => (await b.start()).status, (s) => s === 200, "release on room expiry", 20_000, 1000);
    b.host.close();
  });

  it("drops a listed room from the lobby when its heartbeats stop arriving", async () => {
    const made = await rawRequest(port, "POST", "/api/rooms", { "content-type": "application/json" },
      roomBody({ listed: true, media_sha256: TIDAL_GLASS }));
    const { code, resume_token: token } = JSON.parse(made.text);
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", token, TIDAL_GLASS));
    await host.next(ofType("welcome"), "welcome");
    const lobby = async () => JSON.parse((await rawRequest(port, "GET", "/api/lobby")).text).rooms;
    const listed = await waitFor(lobby, (rooms) => rooms.some((e) => e.code === code), "listing");
    assert.equal(listed.find((e) => e.code === code).title, "Tidal Glass");
    // The default heartbeat (60 s) is longer than this server's 3 s expiry.
    await waitFor(lobby, (rooms) => !rooms.some((e) => e.code === code), "expiry", 15_000, 1000);
    host.close();
  });
});

describe("hosted uploads off (launch setting)", () => {
  let port;
  before(async () => {
    port = await boot({ MEDIA_UPLOADS_ENABLED: 0 });
  });
  const create = (over) => rawRequest(port, "POST", "/api/rooms", { "content-type": "application/json" }, roomBody(over));

  it("says so in /api/info", async () => {
    assert.equal(JSON.parse((await rawRequest(port, "GET", "/api/info")).text).uploads, false);
  });

  it("refuses rooms, imported queues and queue items that are not built-in, and upload starts", async () => {
    const own = await create({ media_sha256: "d".repeat(64), media_extension: "mp4" });
    assert.equal(own.status, 403);
    assert.deepEqual(JSON.parse(own.text), { error: UPLOADS_OFF });
    const imported = await create({ initial_snapshot: { media_sha256: SLOW_ORBIT, queue: [{ sha256: "e".repeat(64) }] } });
    assert.equal(imported.status, 403);
    assert.equal(JSON.parse(imported.text).error, UPLOADS_OFF);

    const made = await create({});
    assert.equal(made.status, 200, made.text);
    const { code, resume_token: token } = JSON.parse(made.text);
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", token, SLOW_ORBIT));
    await host.next(ofType("welcome"), "welcome");
    host.send({ type: "queue_add", sha256: "f".repeat(64), extension: "mp4", title: "Mine", width: 640, height: 360, duration_ms: 1000 });
    assert.equal((await host.next(ofType("error"), "refused queue item")).message, UPLOADS_OFF);
    host.send({ type: "queue_add", sha256: TIDAL_GLASS, extension: "webp", title: "x", width: 640, height: 360, duration_ms: 1000 });
    await host.next((f) => f.type === "snapshot" && f.snapshot.queue.some((i) => i.sha256 === TIDAL_GLASS), "built-in queued");

    const start = await fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media`, {
      method: "POST",
      headers: { authorization: "Bearer anything", "content-type": "application/json" },
      body: JSON.stringify({ sha256: "f".repeat(64), size: 10, part_size: 10 }),
    });
    assert.equal(start.status, 403);
    assert.deepEqual(await start.json(), { error: UPLOADS_OFF });
    host.close();
  });
});
