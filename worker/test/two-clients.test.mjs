// Two-client integration test against `wrangler dev --local`.
// Run through scripts/worker-test.sh, which builds the wasm module first.
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import {
  SHA,
  connect,
  createRoom,
  delay,
  freePort,
  joinMessage,
  ofType,
  startServer,
} from "./helpers.mjs";

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
    const { code, client_id: hostId, resume_token: hostToken } = await createRoom(port);
    assert.match(code, /^[A-HJ-NP-Z2-9]{8}$/);
    assert.ok(hostToken, "creator receives a resume token");

    // Host joins by presenting the secret token; its public id is unchanged.
    const a = await connect(port, code);
    a.send(joinMessage(code, "Host", hostToken));
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
    assert.notEqual(welcomeB.client_id, hostId);
    assert.ok(welcomeB.resume_token, "a new player receives a resume token");
    assert.equal(welcomeB.media_upload_token, null, "only the host gets an upload token");
    const joined = await a.next(ofType("peer_joined"), "peer_joined");
    assert.equal(joined.name, "Bee");

    a.send({ type: "cursor", x: 10, y: 20 });
    const cursor = await b.next(ofType("cursor"), "host cursor");
    assert.equal(cursor.name, "Host");
    assert.equal(cursor.client_id, hostId);
    assert.equal(a.pending(), 0, "a player does not receive their own cursor");
    b.send({ type: "chat", text: " hello\nroom " });
    const chatA = await a.next(ofType("chat"), "chat from Bee");
    const chatB = await b.next(ofType("chat"), "own chat");
    assert.equal(chatA.text, "hello room");
    assert.equal(chatA.id, chatB.id);
    a.send({ type: "ping", x: 12, y: 18 });
    assert.equal((await b.next(ofType("ping"), "board ping")).name, "Host");

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

  it("resumes the solve clock in the first welcome after an empty room", async () => {
    const { code, resume_token } = await createRoom(port);
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", resume_token));
    await host.next(ofType("welcome"), "host welcome");
    host.send({ type: "drag_start", piece_id: 0 });
    await host.next(ofType("pieces_changed"), "clock start");
    host.send({ type: "drag_end", piece_id: 0, x: 300, y: 200 });
    await host.next(ofType("pieces_changed"), "clock drop");
    host.close();
    assert.equal(await host.waitClosed(), true);
    const guest = await connect(port, code);
    guest.send(joinMessage(code, "Guest", undefined, SHA));
    const welcome = await guest.next(ofType("welcome"), "guest welcome");
    assert.equal(welcome.snapshot.stats.started, true);
    assert.equal(typeof welcome.snapshot.stats.running_since_ms, "number");
    guest.close();
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

  it("does not let anyone take the host role with the public id", async () => {
    const { code, client_id: hostId, resume_token: hostToken } = await createRoom(port);
    const guest = await connect(port, code);
    guest.send(joinMessage(code, "Guest"));
    const welcome = await guest.next(ofType("welcome"), "guest welcome");
    assert.equal(welcome.snapshot.host_id, hostId, "the host id is public");

    // Presenting the public id (as a query or a bogus token) grants nothing.
    const thief = await connect(port, code, `&client=${hostId}`);
    thief.send(joinMessage(code, "Thief", "not-the-token"));
    const stolen = await thief.next(ofType("welcome"), "thief welcome");
    assert.notEqual(stolen.client_id, hostId);
    assert.equal(stolen.media_upload_token, null);
    thief.send({ type: "set_repeat", mode: "all" });
    await thief.next(ofType("error"), "thief cannot change host-only settings");

    // The creator, holding the secret, still gets the host identity.
    const creator = await connect(port, code);
    creator.send(joinMessage(code, "Host", hostToken));
    const real = await creator.next(ofType("welcome"), "creator welcome");
    assert.equal(real.client_id, hostId);
    assert.ok(real.media_upload_token, "host receives the upload token");

    // The same token cannot be used twice at once.
    const twin = await connect(port, code);
    twin.send(joinMessage(code, "Twin", hostToken));
    const refused = await twin.next(ofType("error"), "duplicate resume");
    assert.match(refused.message, /already connected/i);
    assert.equal(await twin.waitClosed(), true);
    for (const socket of [guest, thief, creator]) socket.close();
  });

  it("clears ghost players and their leases after a restart", async () => {
    const { code, resume_token: hostToken } = await createRoom(port);
    const a = await connect(port, code);
    a.send(joinMessage(code, "Host", hostToken));
    await a.next(ofType("welcome"), "host welcome");
    const b = await connect(port, code);
    b.send(joinMessage(code, "Bee"));
    await b.next(ofType("welcome"), "guest welcome");
    b.send({ type: "drag_start", piece_id: 3 });
    await b.next(ofType("pieces_changed"), "lease on piece 3");

    // Let the write-behind flush, then kill the server without closing sockets.
    await delay(1500);
    await server.stop();
    port = await freePort();
    server = await startServer(port, persistDir);

    const c = await connect(port, code);
    c.send(joinMessage(code, "Cee"));
    const welcome = await c.next(ofType("welcome"), "welcome after restart");
    assert.deepEqual(welcome.peers, [], "ghost players are gone");
    assert.equal(welcome.snapshot.pieces.find((p) => p.id === 3).held_by, null);
    assert.equal(welcome.snapshot.host_id, welcome.client_id, "the newcomer is promoted");
    c.send({ type: "drag_start", piece_id: 3 });
    await c.next(ofType("pieces_changed"), "piece 3 is free to take");
    c.close();
  });

  it("drops a player whose socket goes silent", async () => {
    const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-worker-idle-"));
    const idlePort = await freePort();
    const idle = await startServer(idlePort, dir, { IDLE_SOCKET_MS: 1500 });
    try {
      const { code, resume_token: hostToken } = await createRoom(idlePort);
      const a = await connect(idlePort, code);
      a.send(joinMessage(code, "Host", hostToken));
      await a.next(ofType("welcome"), "host welcome");
      const b = await connect(idlePort, code);
      b.send(joinMessage(code, "Bee"));
      await b.next(ofType("welcome"), "guest welcome");
      // The host keeps its socket alive with pings; the guest says nothing.
      const keepAlive = setInterval(() => a.send("ping"), 300);
      try {
        const left = await a.next(ofType("peer_left"), "peer_left for the silent guest", 20000);
        assert.equal(left.name, "Bee");
      } finally {
        clearInterval(keepAlive);
      }
      a.close();
    } finally {
      await idle.stop();
    }
  });

  it("keeps spectators out of play and persists claims and timelapse", async () => {
    const { code, resume_token: hostToken } = await createRoom(port, { player_key: "host-key" });
    const host = await connect(port, code);
    host.send({ ...joinMessage(code, "Host", hostToken), player_key: "host-key" });
    const hostWelcome = await host.next(ofType("welcome"), "host welcome");
    assert.equal(hostWelcome.spectator, false);

    const watcher = await connect(port, code);
    watcher.send({ ...joinMessage(code, "Watcher"), spectator: true, player_key: "watch-key" });
    const watchWelcome = await watcher.next(ofType("welcome"), "spectator welcome");
    assert.equal(watchWelcome.spectator, true);
    assert.deepEqual(watchWelcome.peers, ["Host"]);
    assert.equal((await host.next(ofType("peer_joined"), "spectator joined")).spectator, true);
    watcher.send({ type: "drag_start", piece_id: 0 });
    await watcher.next(ofType("error"), "spectator cannot move");
    watcher.send({ type: "ping", x: 1, y: 1 });
    await watcher.next(ofType("error"), "spectator cannot ping");
    watcher.send({ type: "chat", text: "Watching" });
    assert.equal((await host.next(ofType("chat"), "spectator chat")).text, "Watching");

    host.send({ type: "drag_start", piece_id: 0 });
    await host.next(ofType("pieces_changed"), "host lease");
    host.send({ type: "drag_end", piece_id: 0, x: 300, y: 200 });
    await host.next(ofType("pieces_changed"), "host drop");
    host.send({ type: "request_timelapse" });
    const before = await host.next(ofType("timelapse"), "timelapse");
    assert.ok(before.timelapse.frames.length > 0);

    const imposter = await connect(port, code);
    imposter.send({ ...joinMessage(code, "Host"), player_key: "other-key" });
    await imposter.next(ofType("error"), "claimed name refused");
    imposter.close();
    watcher.close();
    host.close();
    await delay(1000);
    await server.stop();
    port = await freePort();
    server = await startServer(port, persistDir);

    const returner = await connect(port, code);
    returner.send({ ...joinMessage(code, "Host", hostToken), player_key: "host-key" });
    const welcome = await returner.next(ofType("welcome"), "host after restart");
    assert.equal(welcome.name, "Host");
    returner.send({ type: "request_timelapse" });
    const after = await returner.next(ofType("timelapse"), "saved timelapse");
    assert.deepEqual(after.timelapse, before.timelapse);
    returner.close();
  });
});
