// Room expiry: unclaimed rooms, idle rooms, and deletion of their R2 media.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { connect, createRoom, delay, freePort, joinMessage, ofType, startServer } from "./helpers.mjs";

const hex = (bytes) => createHash("sha256").update(bytes).digest("hex");
const bearer = (token) => ({ authorization: `Bearer ${token}` });

const blobCount = async (dir) => {
  try {
    return (await readdir(path.join(dir, "v3/r2/pieceoff-media/blobs"))).length;
  } catch {
    return 0;
  }
};

async function waitFor(check, label, ms = 15000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (await check()) return;
    await delay(250);
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function roomExists(port, code) {
  const socket = await connect(port, code);
  const first = await Promise.race([
    socket.next(() => true, "first frame", 3000).catch(() => null),
    delay(1500).then(() => null),
  ]);
  socket.close();
  return !(first && first.type === "error" && first.message === "Room code not found");
}

describe("room cleanup", () => {
  let dir;
  let port;
  let server;

  before(async () => {
    dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-cleanup-state-"));
    port = await freePort();
    server = await startServer(port, dir, {
      FLUSH_MS: 300,
      IDLE_CLEANUP_MS: 2000,
      UNCLAIMED_MS: 1500,
    });
  });
  after(async () => {
    await server?.stop();
    await rm(dir, { recursive: true, force: true });
  });

  const startUpload = (code, token, sha256, size) =>
    fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media`, {
      method: "POST",
      headers: { ...bearer(token), "content-type": "application/json" },
      body: JSON.stringify({ sha256, size, part_size: size }),
    }).then((r) => r.json());

  const putPart = (code, token, uploadId, bytes) =>
    fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media/${uploadId}/parts/1`, {
      method: "PUT",
      headers: { ...bearer(token), "x-part-sha256": hex(bytes) },
      body: bytes,
    });

  it("deletes a room whose creator never connects", async () => {
    const { code } = await createRoom(port);
    assert.equal(await roomExists(port, code), true, "exists right after creation");
    await waitFor(async () => !(await roomExists(port, code)), "unclaimed room to expire");
  });

  it("keeps a room while someone is in it, then deletes it and all its media", async () => {
    const finished = randomBytes(1024 * 1024);
    const partial = randomBytes(1024 * 1024);
    const finishedSha = hex(finished);
    const { code, resume_token: hostToken } = await createRoom(port, { media_sha256: finishedSha });
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", hostToken, finishedSha));
    const welcome = await host.next(ofType("welcome"), "host welcome");
    const token = welcome.media_upload_token;

    // One finished upload and one that is still in flight.
    const first = await startUpload(code, token, finishedSha, finished.length);
    assert.equal((await putPart(code, token, first.upload_id, finished)).status, 200);
    const done = await fetch(
      `http://127.0.0.1:${port}/api/rooms/${code}/media/${first.upload_id}/complete`,
      { method: "POST", headers: bearer(token) },
    );
    assert.equal(done.status, 200);
    host.send({
      type: "queue_add",
      sha256: hex(partial),
      extension: "mp4",
      title: "Second",
      width: 640,
      height: 360,
      duration_ms: 5000,
    });
    await host.next(
      (f) => f.type === "snapshot" && f.snapshot.queue.some((i) => i.sha256 === hex(partial)),
      "second item queued",
    );
    const second = await startUpload(code, token, hex(partial), partial.length);
    assert.equal((await putPart(code, token, second.upload_id, partial)).status, 200);
    assert.ok((await blobCount(dir)) >= 2, "media bytes are stored");

    // Being present is enough: the room outlives the idle period.
    await delay(3500);
    assert.equal(await roomExists(port, code), true, "alive while a player is connected");

    // A short absence does not expire it either.
    host.close();
    await delay(600);
    const back = await connect(port, code);
    back.send(joinMessage(code, "Host", hostToken, finishedSha));
    await back.next(ofType("welcome"), "welcome on return");
    await delay(2500);
    assert.equal(await roomExists(port, code), true, "rejoining resets the idle clock");

    // Leaving for good: the room, its finished media and the unfinished upload go.
    back.close();
    await waitFor(async () => !(await roomExists(port, code)), "idle room to expire");
    await waitFor(async () => (await blobCount(dir)) === 0, "R2 objects to be deleted");
    const gone = await fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media/${finishedSha}`, {
      headers: bearer(hostToken),
    });
    assert.ok([403, 404].includes(gone.status), `media is gone (${gone.status})`);
  });
});

describe("room cleanup retries", () => {
  it("finishes deleting a room after a transient R2 failure", async () => {
    const dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-cleanup-retry-"));
    const retryPort = await freePort();
    const retryServer = await startServer(retryPort, dir, {
      FLUSH_MS: 300,
      IDLE_CLEANUP_MS: 1500,
      FAIL_R2_DELETE_ONCE: 1,
    });
    try {
      const bytes = randomBytes(1024 * 1024);
      const sha = hex(bytes);
      const { code, resume_token } = await createRoom(retryPort, { media_sha256: sha });
      const host = await connect(retryPort, code);
      host.send(joinMessage(code, "Host", resume_token, sha));
      const { media_upload_token: token } = await host.next(ofType("welcome"), "welcome");
      const base = `http://127.0.0.1:${retryPort}/api/rooms/${code}/media`;
      const started = await fetch(base, {
        method: "POST",
        headers: { ...bearer(token), "content-type": "application/json" },
        body: JSON.stringify({ sha256: sha, size: bytes.length, part_size: bytes.length }),
      }).then((r) => r.json());
      await fetch(`${base}/${started.upload_id}/parts/1`, {
        method: "PUT",
        headers: { ...bearer(token), "x-part-sha256": sha },
        body: bytes,
      });
      await fetch(`${base}/${started.upload_id}/complete`, { method: "POST", headers: bearer(token) });
      assert.ok((await blobCount(dir)) >= 1);
      host.close();
      // The first deletion attempt fails on purpose; the retry must finish the job.
      await waitFor(async () => !(await roomExists(retryPort, code)), "room to disappear", 40000);
      await waitFor(async () => (await blobCount(dir)) === 0, "R2 objects to be deleted", 40000);
    } finally {
      await retryServer.stop();
      await rm(dir, { recursive: true, force: true });
    }
  });
});
