// The ONLINE_PLAY_ENABLED=0 emergency switch: lobby reads answer an empty list
// and every other hosted-play route answers 503, without touching a room.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { freePort, startServer } from "./helpers.mjs";

const PAUSED = "PieceOff Online is paused right now. Please try again later.";

describe("online play paused", () => {
  let port;
  let server;
  let dir;
  before(async () => {
    dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-paused-"));
    port = await freePort();
    server = await startServer(port, dir, { ONLINE_PLAY_ENABLED: 0 });
  });
  after(async () => {
    await server?.stop();
    await rm(dir, { recursive: true, force: true });
  });

  it("still answers health and info", async () => {
    assert.equal((await fetch(`http://127.0.0.1:${port}/health`)).status, 200);
    assert.equal((await fetch(`http://127.0.0.1:${port}/api/info`)).status, 200);
  });

  it("returns an empty lobby", async () => {
    const response = await fetch(`http://127.0.0.1:${port}/api/lobby`);
    assert.equal(response.status, 200);
    assert.deepEqual(await response.json(), { rooms: [] });
  });

  it("refuses linking, creating and joining", async () => {
    const calls = [
      fetch(`http://127.0.0.1:${port}/api/links`, { method: "POST", body: "{}" }),
      fetch(`http://127.0.0.1:${port}/api/rooms`, { method: "POST", body: '{"type":"create_room"}' }),
      fetch(`http://127.0.0.1:${port}/ws?code=ABCDEFGH`),
      fetch(`http://127.0.0.1:${port}/api/rooms/ABCDEFGH/media/${"0".repeat(64)}`),
    ];
    for (const response of await Promise.all(calls)) {
      assert.equal(response.status, 503);
      assert.equal((await response.json()).error, PAUSED);
    }
  });
});
