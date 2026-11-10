// The service-wide daily ceilings (GLOBAL_ROOMS_PER_DAY, GLOBAL_LINKS_PER_DAY)
// and GET /api/admin/usage, against a local `wrangler dev`.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { connect, fakeTurnstile, freePort, joinMessage, ofType, rawRequest, startServer } from "./helpers.mjs";

const SITE = "https://pieceoff.app";
const SLOW_ORBIT = "f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80";
const DAILY_LIMIT = "PieceOff Online has reached today's limit. Please try again tomorrow.";
const DAY_MS = 86_400_000;
const isoDay = (ms) => new Date(Math.floor(ms / DAY_MS) * DAY_MS).toISOString().slice(0, 10);

const cleanups = [];
after(async () => {
  for (const cleanup of cleanups.reverse()) await cleanup();
});

const roomBody = (over = {}) => JSON.stringify({
  type: "create_room", name: "Host", title: "Puzzle", media_sha256: SLOW_ORBIT,
  media_extension: "webp", width: 640, height: 360, rows: 3, columns: 4, duration_ms: 5000, ...over,
});
/** Passes admission but fails inside the room: listed rooms take built-in media only. */
const failingBody = roomBody({ listed: true, media_sha256: "b".repeat(64), media_extension: "mp4" });

describe("global daily caps", () => {
  let port;
  const call = (method, url, headers = {}, body = null) => rawRequest(port, method, url, headers, body);
  const usage = async (token = "admin-secret") => {
    const response = await call("GET", "/api/admin/usage", token ? { authorization: `Bearer ${token}` } : {});
    return { status: response.status, body: response.status === 200 ? JSON.parse(response.text) : response.text };
  };
  const createWith = (key, body = roomBody()) =>
    call("POST", "/api/rooms", { "content-type": "application/json", authorization: `Bearer ${key}` }, body);
  const createBrowser = (token, body = roomBody()) =>
    call("POST", "/api/rooms", { "content-type": "application/json", "x-turnstile-token": token }, body);

  before(async () => {
    const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-caps-"));
    port = await freePort();
    const verifier = await fakeTurnstile();
    const server = await startServer(port, dir, {
      DEV_ALLOW_UNVERIFIED_CREATE: 0,
      TURNSTILE_SECRET: "secret",
      TURNSTILE_HOSTNAME: "pieceoff.app",
      TURNSTILE_VERIFY_URL: verifier.url,
      SITE_ORIGIN: SITE,
      ADMIN_TOKEN: "admin-secret",
      DEVICE_ROOMS_PER_DAY: 30,
      GLOBAL_ROOMS_PER_DAY: 3,
      GLOBAL_LINKS_PER_DAY: 2,
    });
    cleanups.push(async () => {
      await server.stop();
      await verifier.close();
      await rm(dir, { recursive: true, force: true });
    });
  });

  async function link() {
    const made = await call("POST", "/api/links", {}, "");
    assert.equal(made.status, 200, made.text);
    const body = JSON.parse(made.text);
    const approved = await call("POST", `/api/links/${body.link_code}/approve`, { origin: SITE, "x-turnstile-token": "good-link-device" });
    assert.equal(approved.status, 200, approved.text);
    const polled = await call("GET", `/api/links/${body.link_code}`, { authorization: `Bearer ${body.poll_secret}` });
    assert.equal(polled.status, 200, polled.text);
    return JSON.parse(polled.text).device_key;
  }

  it("guards GET /api/admin/usage with ADMIN_TOKEN and reports today's counts and caps", async () => {
    assert.equal((await usage("")).status, 403, "no token");
    assert.equal((await usage("wrong")).status, 403, "wrong token");
    assert.equal((await call("POST", "/api/admin/usage", { authorization: "Bearer admin-secret" })).status, 404, "GET only");
    const fresh = await usage();
    assert.equal(fresh.status, 200);
    assert.deepEqual(fresh.body, { day: isoDay(Date.now()), rooms: 0, rooms_cap: 3, links: 0, links_cap: 2 });
  });

  let key;
  it("refuses link requests past the daily ceiling with 503", async () => {
    key = await link();
    await link();
    const refused = await call("POST", "/api/links", {}, "");
    assert.equal(refused.status, 503);
    assert.deepEqual(JSON.parse(refused.text), { error: DAILY_LIMIT });
    assert.equal((await usage()).body.links, 2);
  });

  it("counts device-key and browser rooms together, refunds failed creates, and keeps joins and the lobby working", async () => {
    assert.equal((await createWith(key)).status, 200);
    assert.equal((await createBrowser("bad-token")).status, 403, "unverified browser");
    assert.equal((await usage()).body.rooms, 1, "a refused Turnstile check costs no slot");

    const browser = await createBrowser("good-create-room");
    assert.equal(browser.status, 200, browser.text);
    const { code, resume_token: token } = JSON.parse(browser.text);
    assert.equal((await createWith(key, failingBody)).status, 400);
    assert.equal((await createBrowser("good-create-room", failingBody)).status, 400);
    assert.equal((await usage()).body.rooms, 2, "failed creates give their slot back");

    assert.equal((await createWith(key)).status, 200, "the slots refunded above are usable");
    for (const refused of [await createWith(key), await createBrowser("good-create-room")]) {
      assert.equal(refused.status, 503);
      assert.deepEqual(JSON.parse(refused.text), { error: DAILY_LIMIT });
    }
    const today = await usage();
    assert.deepEqual(today.body, { day: isoDay(Date.now()), rooms: 3, rooms_cap: 3, links: 2, links_cap: 2 });

    assert.equal((await call("GET", "/api/lobby")).status, 200, "lobby reads still work");
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", token, SLOW_ORBIT));
    await host.next(ofType("welcome"), "host welcome");
    const guest = await connect(port, code);
    guest.send(joinMessage(code, "Guest", undefined, SLOW_ORBIT));
    await guest.next(ofType("welcome"), "guest welcome past the cap");
    guest.close();
    host.close();
  });
});

describe("global daily caps roll over at midnight UTC", () => {
  it("starts a new count on the next UTC day and keeps the admin route off without ADMIN_TOKEN", async () => {
    const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-caps-day-"));
    cleanups.push(() => rm(dir, { recursive: true, force: true }));
    const port = await freePort();
    const create = () => rawRequest(port, "POST", "/api/rooms", { "content-type": "application/json" }, roomBody());
    const usage = () => rawRequest(port, "GET", "/api/admin/usage", { authorization: "Bearer admin-secret" });

    let server = await startServer(port, dir, { GLOBAL_ROOMS_PER_DAY: 1 });
    try {
      assert.equal((await usage()).status, 404, "no ADMIN_TOKEN, no admin routes");
      assert.equal((await create()).status, 200);
      assert.equal((await create()).status, 503);
    } finally {
      await server.stop();
    }

    // Same storage, clock one day on (test hook, honoured only with the dev bypass).
    server = await startServer(port, dir, { GLOBAL_ROOMS_PER_DAY: 1, ADMIN_TOKEN: "admin-secret", TEST_CLOCK_OFFSET_MS: DAY_MS });
    try {
      const fresh = JSON.parse((await usage()).text);
      assert.deepEqual(fresh, { day: isoDay(Date.now() + DAY_MS), rooms: 0, rooms_cap: 1, links: 0, links_cap: 500 });
      assert.equal((await create()).status, 200, "a new day has room again");
      assert.equal((await create()).status, 503);
      assert.equal(JSON.parse((await usage()).text).rooms, 1);
    } finally {
      await server.stop();
    }
  });
});
