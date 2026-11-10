// Moderation on the hosted rooms service: host kick with a rejoin ban, chat
// off by default in listed rooms, and the public name filter, against a
// local `wrangler dev`.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { connect, createRoom, freePort, joinMessage, ofType, startServer } from "./helpers.mjs";

const SLOW_ORBIT = "f50f079590abeae947fc87405072a1e8f75cfc8d64694726a75cc5d612f8db80";
const REMOVED = "The host removed you from this room.";
const CHAT_OFF = "The host turned chat off in this room.";
const NAME_REFUSED = "Pick a different name for public rooms";

const builtin = { media_sha256: SLOW_ORBIT, media_extension: "webp" };
const join = (code, name, key, token) => ({
  ...joinMessage(code, name, token, SLOW_ORBIT),
  ...(key ? { player_key: key } : {}),
});

describe("moderation", () => {
  let port;
  let dir;
  let server;

  before(async () => {
    dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-moderation-"));
    port = await freePort();
    server = await startServer(port, dir);
  });

  after(async () => {
    await server?.stop();
    await rm(dir, { recursive: true, force: true });
  });

  async function hostedRoom(overrides = {}) {
    const { code, resume_token: token } = await createRoom(port, { ...builtin, player_key: "host-key", ...overrides });
    const host = await connect(port, code);
    host.send(join(code, overrides.name ?? "Host", "host-key", token));
    const welcome = await host.next(ofType("welcome"), "host welcome");
    return { code, host, welcome };
  }

  it("lets the host remove a player, closes their socket and refuses them back", async () => {
    const { code, host } = await hostedRoom();
    const troll = await connect(port, code);
    troll.send(join(code, "Troll", "troll-key"));
    const { resume_token: trollToken } = await troll.next(ofType("welcome"), "troll welcome");
    await host.next(ofType("peer_joined"), "troll joined");
    const bo = await connect(port, code);
    bo.send(join(code, "Bo", "bo-key"));
    await bo.next(ofType("welcome"), "bo welcome");
    troll.send({ type: "drag_start", piece_id: 0 });
    await host.next((f) => f.type === "pieces_changed" && f.pieces.some((p) => p.held_by), "troll's lease");

    bo.send({ type: "kick_player", name: "Troll" });
    assert.equal((await bo.next(ofType("error"), "guest refused")).message, "Only the host can remove a player");
    host.send({ type: "kick_player", name: "host" });
    assert.equal((await host.next(ofType("error"), "self refused")).message, "You can't remove yourself");

    host.send({ type: "kick_player", name: "troll" });
    const notice = await troll.next(ofType("kicked"), "kicked notice");
    assert.equal(notice.message, REMOVED);
    assert.ok(await troll.waitClosed(), "the removed player's socket is closed");
    assert.equal((await host.next(ofType("peer_left"), "peer_left")).name, "Troll");
    const freed = await bo.next(
      (f) => f.type === "snapshot" && f.snapshot.pieces.every((p) => p.held_by === null),
      "lease released",
    );
    assert.ok(freed);

    // Not with the old resume token, not under a new name with the same key,
    // not under the name with a new key, and not as a spectator.
    for (const [name, key, token, spectator] of [
      ["Troll", "troll-key", trollToken, false],
      ["Fresh Face", "troll-key", undefined, false],
      ["troll", "new-key", undefined, false],
      ["Troll", undefined, undefined, true],
    ]) {
      const again = await connect(port, code);
      again.send({ ...join(code, name, key, token), spectator });
      assert.equal((await again.next(ofType("error"), `refusal for ${name}`)).message, REMOVED);
      assert.ok(await again.waitClosed(), "a refused rejoin is closed");
    }
    // Everyone else carries on.
    bo.send({ type: "chat", text: "still here" });
    await host.next(ofType("chat"), "chat after the kick");
    host.close();
    bo.close();

    // The ban is saved with the room: it holds after a restart.
    await bo.waitClosed();
    await server.stop();
    server = await startServer(port, dir);
    const afterRestart = await connect(port, code);
    afterRestart.send(join(code, "Another Name", "troll-key"));
    assert.equal((await afterRestart.next(ofType("error"), "refusal after restart")).message, REMOVED);
  });

  it("starts listed rooms with chat off, refuses chat until the host turns it on, and keeps pings", async () => {
    const { code, host, welcome } = await hostedRoom({ listed: true });
    assert.equal(welcome.snapshot.listed, true);
    assert.equal(welcome.snapshot.chat_enabled, false);
    const guest = await connect(port, code);
    guest.send(join(code, "Guest", "guest-key"));
    await guest.next(ofType("welcome"), "guest welcome");
    guest.send({ type: "chat", text: "hello?" });
    assert.equal((await guest.next(ofType("error"), "chat refused")).message, CHAT_OFF);
    guest.send({ type: "ping", x: 10, y: 10 });
    assert.equal((await host.next(ofType("ping"), "ping still relayed")).name, "Guest");

    guest.send({ type: "set_chat_enabled", enabled: true });
    await guest.next(ofType("error"), "only the host switches chat");
    host.send({ type: "set_chat_enabled", enabled: true });
    const on = await guest.next((f) => f.type === "snapshot" && f.snapshot.chat_enabled, "chat on");
    assert.equal(on.snapshot.chat_enabled, true);
    guest.send({ type: "chat", text: "hi all" });
    assert.equal((await host.next(ofType("chat"), "chat relayed")).text, "hi all");

    // Off again: newcomers do not get the earlier lines.
    host.send({ type: "set_chat_enabled", enabled: false });
    await guest.next((f) => f.type === "snapshot" && !f.snapshot.chat_enabled, "chat off");
    const late = await connect(port, code);
    late.send(join(code, "Late", "late-key"));
    await late.next(ofType("welcome"), "late welcome");
    host.send({ type: "set_chat_enabled", enabled: true });
    await late.next((f) => f.type === "snapshot" && f.snapshot.chat_enabled, "chat on for late");
    await assert.rejects(late.next(ofType("chat_history"), "no history", 300));
    host.close();
    guest.close();
    late.close();
  });

  it("refuses blocked names in listed rooms only", async () => {
    const refused = await fetch(`http://127.0.0.1:${port}/api/rooms`, {
      method: "POST",
      body: JSON.stringify({
        type: "create_room", name: "p0rn king", title: "Puzzle", ...builtin,
        width: 640, height: 360, rows: 3, columns: 4, listed: true,
      }),
    });
    assert.equal(refused.status, 400);
    assert.match(await refused.text(), new RegExp(NAME_REFUSED));

    const { code, host } = await hostedRoom({ listed: true });
    const bad = await connect(port, code);
    bad.send(join(code, "sexy", "k1"));
    assert.equal((await bad.next(ofType("error"), "name refused")).message, NAME_REFUSED);
    const fine = await connect(port, code);
    fine.send(join(code, "Peacock", "k2"));
    await fine.next(ofType("welcome"), "ordinary name");
    host.close();
    fine.close();

    const unlisted = await hostedRoom();
    const anyName = await connect(port, unlisted.code);
    anyName.send(join(unlisted.code, "sexy", "k3"));
    await anyName.next(ofType("welcome"), "invite rooms are not filtered");
    unlisted.host.close();
    anyName.close();
  });
});
