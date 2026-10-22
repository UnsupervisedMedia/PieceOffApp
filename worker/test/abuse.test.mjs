// Admission control: Turnstile, rate limits, origins, socket caps.
import assert from "node:assert/strict";
import http from "node:http";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { connect, createBody, delay, freePort, joinMessage, ofType, rawRequest, startServer } from "./helpers.mjs";

const cleanups = [];
after(async () => {
  for (const cleanup of cleanups.reverse()) await cleanup();
});

async function boot(vars) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-abuse-"));
  const port = await freePort();
  const server = await startServer(port, dir, vars);
  cleanups.push(async () => {
    await server.stop();
    await rm(dir, { recursive: true, force: true });
  });
  return port;
}

/** A stand-in for Cloudflare's siteverify endpoint. */
async function fakeVerifier() {
  const port = await freePort();
  const server = http.createServer((req, res) => {
    let data = "";
    req.on("data", (c) => (data += c));
    req.on("end", () => {
      const token = new URLSearchParams(data).get("response");
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ success: token === "good-token" }));
    });
  });
  await new Promise((resolve) => server.listen(port, "127.0.0.1", resolve));
  cleanups.push(() => new Promise((resolve) => server.close(resolve)));
  return `http://127.0.0.1:${port}/verify`;
}

const post = (port, headers = {}) =>
  rawRequest(port, "POST", "/api/rooms", { "content-type": "application/json", ...headers }, createBody);

describe("room creation gate", () => {
  it("requires a valid Turnstile token and fails closed", async () => {
    const verifyUrl = await fakeVerifier();
    const port = await boot({
      DEV_ALLOW_UNVERIFIED_CREATE: 0,
      TURNSTILE_SECRET: "secret",
      TURNSTILE_VERIFY_URL: verifyUrl,
    });
    assert.equal((await post(port)).status, 403, "no token");
    assert.equal((await post(port, { "x-turnstile-token": "bad-token" })).status, 403);
    const good = await post(port, { "x-turnstile-token": "good-token" });
    assert.equal(good.status, 200);
    assert.match(JSON.parse(good.text).code, /^[A-HJ-NP-Z2-9]{8}$/);
  });

  it("denies creation when the verifier is unreachable", async () => {
    const dead = await freePort();
    const port = await boot({
      DEV_ALLOW_UNVERIFIED_CREATE: 0,
      TURNSTILE_SECRET: "secret",
      TURNSTILE_VERIFY_URL: `http://127.0.0.1:${dead}/verify`,
    });
    assert.equal((await post(port, { "x-turnstile-token": "good-token" })).status, 403);
  });

  it("denies creation when no verifier is configured and the dev bypass is off", async () => {
    const port = await boot({ DEV_ALLOW_UNVERIFIED_CREATE: 0 });
    assert.equal((await post(port, { "x-turnstile-token": "good-token" })).status, 403);
  });

  it("limits how many rooms one address can create per minute", async () => {
    const port = await boot({});
    const statuses = [];
    for (let i = 0; i < 12; i++) statuses.push((await post(port)).status);
    assert.equal(statuses.filter((s) => s === 200).length, 10, JSON.stringify(statuses));
    assert.deepEqual(statuses.slice(10), [429, 429]);
  });
});

describe("browser origins", () => {
  it("refuses unlisted origins on the API and the socket, allows own and listed ones", async () => {
    const port = await boot({ ALLOWED_ORIGINS: "https://play.example" });
    assert.equal((await post(port, { origin: "https://evil.example" })).status, 403);
    assert.equal((await post(port, { origin: `http://127.0.0.1:${port}` })).status, 200);
    const listed = await post(port, { origin: "https://play.example" });
    assert.equal(listed.status, 200);
    const { code } = JSON.parse(listed.text);

    const evilSocket = new WebSocket(`ws://127.0.0.1:${port}/ws?code=${code}`, {
      headers: { Origin: "https://evil.example" },
    });
    const outcome = await new Promise((resolve) => {
      evilSocket.addEventListener("open", () => resolve("open"));
      evilSocket.addEventListener("error", () => resolve("refused"));
    });
    assert.equal(outcome, "refused");
  });
});

describe("socket limits", () => {
  it("budgets lookups of unknown room codes per address", async () => {
    const port = await boot({});
    const statuses = [];
    for (let i = 0; i < 25; i++) {
      const code = "ABCDEFGH".replace(/H$/, "ABCDEFGHJKLMNPQRSTUVWXYZ"[i]);
      statuses.push((await rawRequest(port, "GET", `/ws?code=${code}`)).status);
    }
    assert.ok(statuses.includes(429), `expected a 429 in ${JSON.stringify(statuses)}`);
    assert.ok(statuses.slice(0, 15).every((s) => s !== 429), "early lookups are allowed");
  });

  it("caps unjoined sockets per address and drops ones that never join", async () => {
    const port = await boot({ PREJOIN_MS: 1500 });
    const { code } = JSON.parse((await post(port)).text);
    const sockets = [];
    let refused = 0;
    for (let i = 0; i < 9; i++) {
      try {
        sockets.push(await connect(port, code));
      } catch {
        refused++;
      }
    }
    assert.ok(refused >= 1, `expected some connections to be refused (got ${refused})`);
    assert.ok(sockets.length <= 8, `at most 8 sockets are accepted per address (got ${sockets.length})`);
    // None of them said anything: they are closed shortly after PREJOIN_MS.
    const results = await Promise.all(sockets.map((s) => s.waitClosed()));
    assert.ok(results.every(Boolean), "silent sockets are dropped");
  });
});
