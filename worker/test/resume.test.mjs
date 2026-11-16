// A resume token presented while its player's old socket is still attached
// (a phone that slept, a network cut whose close never arrived) replaces that
// socket at once instead of waiting for the idle sweep. Own server: the
// replaced sockets can linger in wrangler's local proxy and count against the
// per-address socket cap of a shared one.
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { connect, createRoom, freePort, joinMessage, ofType, startServer } from "./helpers.mjs";

describe("resume over a live socket", () => {
  let port;
  let server;

  before(async () => {
    const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-worker-resume-"));
    port = await freePort();
    server = await startServer(port, dir);
  });

  after(async () => {
    await server?.stop();
  });

  it("a valid resume token replaces the player's old socket at once and frees its leases", async () => {
    const { code, client_id: hostId, resume_token: hostToken } = await createRoom(port);
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", hostToken));
    await host.next(ofType("welcome"), "host welcome");
    const old = await connect(port, code);
    old.send(joinMessage(code, "Bee"));
    const first = await old.next(ofType("welcome"), "guest welcome");
    await host.next(ofType("peer_joined"), "guest joined");
    old.send({ type: "drag_start", piece_id: 3 });
    const held = await host.next(ofType("pieces_changed"), "guest lease");
    assert.equal(held.pieces.find((p) => p.id === 3).held_by, first.client_id);

    // The old socket is still attached (a phone that slept): the token wins at once.
    const fresh = await connect(port, code);
    fresh.send(joinMessage(code, "Bee", first.resume_token));
    const welcome = await fresh.next(ofType("welcome"), "resumed welcome", 5000);
    assert.equal(welcome.client_id, first.client_id, "same identity");
    assert.equal(welcome.snapshot.host_id, hostId, "the host is unchanged");
    assert.equal(welcome.snapshot.pieces.find((p) => p.id === 3).held_by, null, "the old lease is released");
    const told = await old.next(ofType("error"), "old socket told");
    assert.match(told.message, /opened in another window/);
    assert.equal(await old.waitClosed(), true);
    // wrangler's local proxy may leave it in CLOSING without the frame; when it arrives it says why.
    if (old.closeInfo()) assert.deepEqual(old.closeInfo(), { code: 1008, reason: "replaced" });
    // Everyone else sees the lease go before the player is back.
    const released = await host.next(ofType("snapshot"), "lease released for the room");
    assert.equal(released.snapshot.pieces.find((p) => p.id === 3).held_by, null);
    await host.next(ofType("peer_joined"), "player back");
    // The resumed connection plays normally.
    fresh.send({ type: "drag_start", piece_id: 3 });
    const regrab = await fresh.next(ofType("pieces_changed"), "lease again");
    assert.equal(regrab.pieces.find((p) => p.id === 3).held_by, first.client_id);
    fresh.send({ type: "drag_end", piece_id: 3, x: 300, y: 200 });
    await fresh.next(ofType("pieces_changed"), "drop");
    for (const socket of [host, fresh]) socket.close();
  });

  it("a host resuming over its own old socket stays host", async () => {
    const { code, client_id: hostId, resume_token: hostToken } = await createRoom(port);
    const old = await connect(port, code);
    old.send(joinMessage(code, "Host", hostToken));
    await old.next(ofType("welcome"), "host welcome");
    const fresh = await connect(port, code);
    fresh.send(joinMessage(code, "Host", hostToken));
    const welcome = await fresh.next(ofType("welcome"), "resumed host", 5000);
    assert.equal(welcome.client_id, hostId);
    assert.equal(welcome.snapshot.host_id, hostId);
    assert.equal(await old.waitClosed(), true);
    if (old.closeInfo()) assert.equal(old.closeInfo().reason, "replaced");
    fresh.close();
  });

  it("the same player key under a new identity still replaces the old window", async () => {
    const { code, resume_token: hostToken } = await createRoom(port, { player_key: "host-key" });
    const host = await connect(port, code);
    host.send({ ...joinMessage(code, "Host", hostToken), player_key: "host-key" });
    await host.next(ofType("welcome"), "host welcome");
    const old = await connect(port, code);
    old.send({ ...joinMessage(code, "Bee"), player_key: "bee-key" });
    const first = await old.next(ofType("welcome"), "bee welcome");
    const other = await connect(port, code);
    other.send({ ...joinMessage(code, "Bee"), player_key: "bee-key" });
    const second = await other.next(ofType("welcome"), "bee in another window");
    assert.notEqual(second.client_id, first.client_id, "no token: a new identity");
    const told = await old.next(ofType("error"), "old window told");
    assert.match(told.message, /opened in another window/);
    assert.equal(await old.waitClosed(), true);
    if (old.closeInfo()) assert.equal(old.closeInfo().reason, "replaced");
    for (const socket of [host, other]) socket.close();
  });
});
