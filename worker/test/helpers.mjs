// Shared helpers for the worker integration tests (local `wrangler dev`).
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WORKER_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const SHA = "a".repeat(64);

export function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

export const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function startServer(port, persistDir, vars = {}) {
  const varArgs = Object.entries({ FLUSH_MS: 500, ...vars }).flatMap(([k, v]) => ["--var", `${k}:${v}`]);
  const child = spawn(
    "npx",
    ["wrangler", "dev", "--local", "--port", String(port), "--inspector-port", "0", "--persist-to", persistDir, ...varArgs],
    {
      cwd: WORKER_DIR,
      detached: true,
      stdio: process.env.DEBUG_WRANGLER ? "inherit" : "ignore",
      env: { ...process.env, WRANGLER_SEND_METRICS: "false", CI: "1" },
    },
  );
  const stop = async () => {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      // already gone
    }
    await delay(500);
  };
  for (let attempt = 0; attempt < 240; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`);
      if (response.ok) return { stop };
    } catch {
      // not up yet
    }
    await delay(250);
  }
  await stop();
  throw new Error("wrangler dev did not become healthy");
}

/** A WebSocket with an inbox, so tests can wait for specific frames. */
export async function connect(port, code, query = "") {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws?code=${code}${query}`);
  const inbox = [];
  const waiters = [];
  let closed = false;
  const pump = () => {
    for (const waiter of [...waiters]) {
      const index = inbox.findIndex(waiter.predicate);
      if (index >= 0) {
        const [frame] = inbox.splice(index, 1);
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(frame);
      }
    }
  };
  ws.addEventListener("message", (event) => {
    if (event.data === "pong") return;
    inbox.push(JSON.parse(event.data));
    pump();
  });
  ws.addEventListener("close", () => {
    closed = true;
  });
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", () => reject(new Error("socket error")), { once: true });
  });
  return {
    send: (message) => ws.send(typeof message === "string" ? message : JSON.stringify(message)),
    next: (predicate, label = "frame", ms = 8000) =>
      new Promise((resolve, reject) => {
        const waiter = { predicate, resolve };
        waiters.push(waiter);
        pump();
        setTimeout(() => {
          if (waiters.includes(waiter)) {
            waiters.splice(waiters.indexOf(waiter), 1);
            reject(new Error(`timed out waiting for ${label}; inbox: ${JSON.stringify(inbox)}`));
          }
        }, ms);
      }),
    pending: () => inbox.length,
    isClosed: () => closed,
    close: () => ws.close(),
    waitClosed: async () => {
      for (let i = 0; i < 80 && !closed; i++) await delay(100);
      return closed;
    },
  };
}

export const ofType = (type) => (frame) => frame.type === type;

export async function createRoom(port, overrides = {}) {
  const response = await fetch(`http://127.0.0.1:${port}/api/rooms`, {
    method: "POST",
    body: JSON.stringify({
      type: "create_room",
      name: "Host",
      title: "Puzzle",
      media_sha256: SHA,
      media_extension: "mp4",
      width: 640,
      height: 360,
      rows: 3,
      columns: 4,
      duration_ms: 5000,
      ...overrides,
    }),
  });
  assert.equal(response.status, 200);
  return response.json();
}

export const joinMessage = (code, name, resumeToken, sha = SHA) => ({
  type: "join_room",
  invite_code: code,
  name,
  media_sha256: sha,
  media_extension: "mp4",
  ...(resumeToken ? { resume_token: resumeToken } : {}),
});

