// R2 media upload/download through `wrangler dev --local`.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { connect, createRoom, freePort, joinMessage, ofType, startServer } from "./helpers.mjs";

const MIB = 1024 * 1024;
const PART = 5 * MIB;
const FILE = randomBytes(12 * MIB); // parts of 5, 5 and 2 MiB
const FILE_SHA = createHash("sha256").update(FILE).digest("hex");
const partOf = (n) => FILE.subarray((n - 1) * PART, Math.min(n * PART, FILE.length));
const hex = (bytes) => createHash("sha256").update(bytes).digest("hex");

const bearer = (token) => ({ authorization: `Bearer ${token}` });

describe("room media", () => {
  let port;
  let dir;
  let server;
  let base;

  before(async () => {
    dir = await mkdtemp(path.join(os.tmpdir(), "pieceoff-media-state-"));
    port = await freePort();
    server = await startServer(port, dir);
  });
  after(async () => {
    await server?.stop();
    await rm(dir, { recursive: true, force: true });
  });

  const api = (code, suffix, init = {}) =>
    fetch(`http://127.0.0.1:${port}/api/rooms/${code}/media${suffix}`, init);

  async function hostedRoom() {
    const { code, resume_token: hostToken } = await createRoom(port, {
      media_sha256: FILE_SHA,
    });
    const host = await connect(port, code);
    host.send(joinMessage(code, "Host", hostToken, FILE_SHA));
    const welcome = await host.next(ofType("welcome"), "host welcome");
    assert.ok(welcome.media_upload_token, "host is given the upload token");
    return { code, host, hostToken, uploadToken: welcome.media_upload_token };
  }

  it("uploads in parts, resumes after a restart, verifies hashes, and serves ranges", async () => {
    const { code, host, hostToken, uploadToken } = await hostedRoom();
    const guest = await connect(port, code);
    guest.send(joinMessage(code, "Guest", undefined, FILE_SHA));
    const guestWelcome = await guest.next(ofType("welcome"), "guest welcome");
    assert.equal(guestWelcome.snapshot.queue[0].available, false);
    const guestToken = guestWelcome.resume_token;

    // Start.
    const started = await api(code, "", {
      method: "POST",
      headers: { ...bearer(uploadToken), "content-type": "application/json" },
      body: JSON.stringify({ sha256: FILE_SHA, size: FILE.length, part_size: PART }),
    });
    assert.equal(started.status, 200);
    const start = await started.json();
    assert.deepEqual(start.missing, [1, 2, 3]);
    assert.equal(start.part_count, 3);

    const putPart = (n, bytes, claimedHash = hex(bytes), token = uploadToken) =>
      api(code, `/${start.upload_id}/parts/${n}`, {
        method: "PUT",
        headers: { ...bearer(token), "x-part-sha256": claimedHash },
        body: bytes,
      });

    // Part 1 fine; part 2 with a wrong hash is rejected and not recorded.
    assert.equal((await putPart(1, partOf(1))).status, 200);
    const tampered = await putPart(2, partOf(2), "0".repeat(64));
    assert.equal(tampered.status, 400);
    // A guest's token cannot upload.
    assert.equal((await putPart(2, partOf(2), undefined, guestToken)).status, 403);
    // A part of the wrong size is refused before it is stored.
    assert.equal((await putPart(3, partOf(2))).status, 400);

    const status = await api(code, `/${start.upload_id}`, { headers: bearer(uploadToken) });
    assert.deepEqual((await status.json()).missing, [2, 3]);

    // Everyone drops and the worker restarts mid-upload.
    host.close();
    guest.close();
    await server.stop();
    port = await freePort();
    server = await startServer(port, dir);

    const host2 = await connect(port, code);
    host2.send(joinMessage(code, "Host", hostToken, FILE_SHA));
    const welcome2 = await host2.next(ofType("welcome"), "host welcome after restart");
    const token2 = welcome2.media_upload_token;
    assert.ok(token2);

    const resumed = await api(code, "", {
      method: "POST",
      headers: { ...bearer(token2), "content-type": "application/json" },
      body: JSON.stringify({ sha256: FILE_SHA, size: FILE.length, part_size: PART }),
    });
    const resume = await resumed.json();
    assert.equal(resume.upload_id, start.upload_id, "the same upload is resumed");
    assert.deepEqual(resume.missing, [2, 3]);

    const put2 = (n, bytes) =>
      api(code, `/${start.upload_id}/parts/${n}`, {
        method: "PUT",
        headers: { ...bearer(token2), "x-part-sha256": hex(bytes) },
        body: bytes,
      });
    assert.equal((await put2(2, partOf(2))).status, 200);

    // Completing early is refused and names what is missing.
    const early = await api(code, `/${start.upload_id}/complete`, {
      method: "POST",
      headers: bearer(token2),
    });
    assert.equal(early.status, 409);
    assert.deepEqual((await early.json()).missing, [3]);

    assert.equal((await put2(3, partOf(3))).status, 200);
    const done = await api(code, `/${start.upload_id}/complete`, {
      method: "POST",
      headers: bearer(token2),
    });
    assert.equal(done.status, 200);
    const available = await host2.next(
      (f) => f.type === "snapshot" && f.snapshot.queue[0].available,
      "snapshot with the media available",
    );
    assert.equal(available.snapshot.queue[0].sha256, FILE_SHA);

    // Download needs a player's resume token.
    const noAuth = await api(code, `/${FILE_SHA}`);
    assert.ok([401, 403].includes(noAuth.status));
    const full = await api(code, `/${FILE_SHA}`, { headers: bearer(hostToken) });
    assert.equal(full.status, 200);
    assert.equal(hex(Buffer.from(await full.arrayBuffer())), FILE_SHA);

    const ranged = await api(code, `/${FILE_SHA}`, {
      headers: { ...bearer(hostToken), range: "bytes=5-9" },
    });
    assert.equal(ranged.status, 206);
    assert.equal(ranged.headers.get("content-range"), `bytes 5-9/${FILE.length}`);
    assert.deepEqual(Buffer.from(await ranged.arrayBuffer()), FILE.subarray(5, 10));

    // Another room's player cannot read this room's media.
    const other = await createRoom(port);
    const cross = await api(code, `/${FILE_SHA}`, { headers: bearer(other.resume_token) });
    assert.ok([401, 403].includes(cross.status));
    const wrongRoom = await api(other.code, `/${FILE_SHA}`, { headers: bearer(hostToken) });
    assert.ok([401, 403, 404].includes(wrongRoom.status));
    host2.close();
  });

  it("enforces the room quota across in-flight uploads and the per-file limit", async () => {
    const dir2 = await mkdtemp(path.join(os.tmpdir(), "pieceoff-media-quota-"));
    const smallPort = await freePort();
    const small = await startServer(smallPort, dir2, { ROOM_MEDIA_QUOTA_BYTES: 16 * MIB });
    try {
      const { code, resume_token } = await createRoom(smallPort, { media_sha256: FILE_SHA });
      const host = await connect(smallPort, code);
      host.send(joinMessage(code, "Host", resume_token, FILE_SHA));
      const { media_upload_token: token } = await host.next(ofType("welcome"), "welcome");
      const start = (sha256, size) =>
        fetch(`http://127.0.0.1:${smallPort}/api/rooms/${code}/media`, {
          method: "POST",
          headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
          body: JSON.stringify({ sha256, size, part_size: PART }),
        });
      assert.equal((await start(FILE_SHA, FILE.length)).status, 200, "12 MiB fits in 16");
      // 12 MiB are now reserved by the unfinished upload; another 12 does not fit.
      assert.equal((await start("b".repeat(64), FILE.length)).status, 507);
      // A file larger than the whole quota is refused outright.
      assert.equal((await start("c".repeat(64), 17 * MIB)).status, 413);
      host.close();
    } finally {
      await small.stop();
    }
  });

  it("rejects malformed media requests", async () => {
    const { code, host, uploadToken } = await hostedRoom();
    const noToken = await api(code, "", { method: "POST", body: "{}" });
    assert.equal(noToken.status, 403);
    const badSha = await api(code, "", {
      method: "POST",
      headers: { ...bearer(uploadToken), "content-type": "application/json" },
      body: JSON.stringify({ sha256: "../etc", size: 10, part_size: 10 }),
    });
    assert.equal(badSha.status, 400);
    const unknownRoom = await api("ZZZZZZZZ", "", { method: "POST", body: "{}" });
    assert.ok([403, 404].includes(unknownRoom.status));
    host.close();
  });
});
