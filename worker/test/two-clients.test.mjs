// Two-client integration test against `wrangler dev --local`.
// Run through scripts/worker-test.sh, which builds the wasm module first.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const WORKER_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SHA = "a".repeat(64);

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function startServer(port, persistDir) {
  const child = spawn(
    "npx",
    ["wrangler", "dev", "--local", "--port", String(port), "--inspector-port", "0", "--persist-to", persistDir],
    {
      cwd: WORKER_DIR,
      detached: true,
      stdio: "ignore",
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
async function connect(port, code, clientId) {
  const query = `code=${code}` + (clientId ? `&client=${clientId}` : "");
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws?${query}`);
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

const ofType = (type) => (frame) => frame.type === type;

async function createRoom(port) {
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
    }),
  });
  assert.equal(response.status, 200);
  return response.json();
}

const joinMessage = (code, name) => ({
  type: "join_room",
  invite_code: code,
  name,
  media_sha256: SHA,
  media_extension: "mp4",
});

describe("room worker", () => {
  let port;
  let persistDir;
  let server;

  before(async () => {
    persistDir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-worker-state-"));
    port = await freePort();
    server = await startServer(port, persistDir);
  });

  after(async () => {
    await server?.stop();
  });

  it("plays a two-client session: join, lease, drop, disconnect, bad frames, restart", async () => {
    const { code, client_id: hostId } = await createRoom(port);
    assert.match(code, /^[A-Z2-9]{8}$/);

    // Host joins with the id it was given.
    const a = await connect(port, code, hostId);
    a.send(joinMessage(code, "Host"));
    const welcomeA = await a.next(ofType("welcome"), "host welcome");
    assert.equal(welcomeA.client_id, hostId);
    assert.deepEqual(welcomeA.peers, []);
    assert.equal(welcomeA.snapshot.invite_code, code);
    assert.equal(welcomeA.snapshot.host_id, hostId);

    // A second player joins and the host hears about it.
    const b = await connect(port, code);
    b.send(joinMessage(code, "Bee"));
    const welcomeB = await b.next(ofType("welcome"), "guest welcome");
    assert.deepEqual(welcomeB.peers, ["Host"]);
    assert.equal(welcomeB.media_upload_token, null, "only the host gets an upload token");
    const joined = await a.next(ofType("peer_joined"), "peer_joined");
    assert.equal(joined.name, "Bee");

    // A takes a piece; both see the lease; B cannot take it.
    a.send({ type: "drag_start", piece_id: 0 });
    const leaseA = await a.next(ofType("pieces_changed"), "lease for A");
    const leaseB = await b.next(ofType("pieces_changed"), "lease for B");
    assert.equal(leaseA.revision, leaseB.revision);
    assert.equal(leaseA.pieces.find((p) => p.id === 0).held_by, hostId);
    b.send({ type: "drag_start", piece_id: 0 });
    await b.next(ofType("error"), "refusal for B");

    // A drops it; both see the move in order.
    a.send({ type: "drag_end", piece_id: 0, x: 300, y: 200 });
    const dropA = await a.next(ofType("pieces_changed"), "drop for A");
    const dropB = await b.next(ofType("pieces_changed"), "drop for B");
    assert.equal(dropA.revision, dropB.revision);
    assert.ok(dropA.revision > leaseA.revision);
    const dropped = dropA.pieces.find((p) => p.id === 0);
    assert.equal(dropped.held_by, null);

    // B takes another piece and vanishes: A sees peer_left, then the release.
    b.send({ type: "drag_start", piece_id: 1 });
    await b.next(ofType("pieces_changed"), "lease for B piece 1");
    await a.next(ofType("pieces_changed"), "A sees B's lease");
    b.close();
    const left = await a.next(ofType("peer_left"), "peer_left");
    assert.equal(left.name, "Bee");
    const released = await a.next(ofType("snapshot"), "snapshot after leave");
    assert.equal(released.snapshot.pieces.find((p) => p.id === 1).held_by, null);

    // Bad frames get an error and the socket keeps working.
    a.send("this is not json");
    await a.next(ofType("error"), "error for garbage");
    a.send({ type: "create_room" });
    await a.next(ofType("error"), "error for create on a socket");
    a.send({ type: "drag_start", piece_id: 2 });
    await a.next(ofType("pieces_changed"), "still usable after bad frames");
    a.send({ type: "drag_end", piece_id: 2, x: 400, y: 100 });
    const lastDrop = await a.next(ofType("pieces_changed"), "final drop");
    const piece2 = lastDrop.pieces.find((p) => p.id === 2);

    // Last player leaves (flushes to storage), the worker restarts, the room survives.
    a.close();
    await delay(1000);
    await server.stop();
    port = await freePort();
    server = await startServer(port, persistDir);
    const again = await connect(port, code);
    again.send(joinMessage(code, "Back"));
    const welcomeAgain = await again.next(ofType("welcome"), "welcome after restart");
    const restored = welcomeAgain.snapshot.pieces.find((p) => p.id === 2);
    assert.equal(restored.x, piece2.x);
    assert.equal(restored.y, piece2.y);
    assert.equal(welcomeAgain.snapshot.invite_code, code);
    again.close();
  });

  it("answers an unknown room code with an error frame and a clean close", async () => {
    const socket = await connect(port, "ZZZZZZZZ");
    const error = await socket.next(ofType("error"), "room-not-found error");
    assert.equal(error.message, "Room code not found");
    assert.equal(await socket.waitClosed(), true);
  });

  it("rejects a malformed room code before touching a room", async () => {
    const response = await fetch(`http://127.0.0.1:${port}/ws?code=nope`);
    assert.equal(response.status, 404);
  });

  it("refuses creation requests that are not create_room messages", async () => {
    const response = await fetch(`http://127.0.0.1:${port}/api/rooms`, {
      method: "POST",
      body: JSON.stringify({ type: "next_item" }),
    });
    assert.equal(response.status, 400);
  });
});
