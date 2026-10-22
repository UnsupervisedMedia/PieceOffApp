import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  checkPart,
  expectedPartSize,
  missingParts,
  planUpload,
  recordPart,
} from "../src/media-state.ts";

const SHA = "a".repeat(64);
const limits = {
  quotaBytes: 1000,
  maxFileBytes: 800,
  minPartBytes: 10,
  maxPartBytes: 100,
  maxParts: 5,
};
const usage = { completedBytes: 0, reservedBytes: 0 };

describe("planUpload", () => {
  it("splits a file into equal parts with a short last part", () => {
    assert.deepEqual(planUpload({ sha256: SHA, size: 100, partSize: 40 }, limits, usage), {
      ok: true,
      partCount: 3,
    });
  });

  it("allows a file smaller than the minimum part size as a single part", () => {
    assert.deepEqual(planUpload({ sha256: SHA, size: 4, partSize: 40 }, limits, usage), {
      ok: true,
      partCount: 1,
    });
  });

  it("rejects a bad fingerprint, empty file and oversized file", () => {
    const bad = planUpload({ sha256: "nope", size: 10, partSize: 40 }, limits, usage);
    assert.deepEqual(bad, { ok: false, status: 400, error: "Invalid media fingerprint" });
    const empty = planUpload({ sha256: SHA, size: 0, partSize: 40 }, limits, usage);
    assert.equal(empty.ok, false);
    assert.equal(empty.status, 400);
    const huge = planUpload({ sha256: SHA, size: 801, partSize: 100 }, limits, usage);
    assert.equal(huge.ok, false);
    assert.equal(huge.status, 413);
  });

  it("rejects part sizes outside the allowed range when more than one part is needed", () => {
    for (const partSize of [5, 101]) {
      const result = planUpload({ sha256: SHA, size: 300, partSize }, limits, usage);
      assert.equal(result.ok, false, `partSize ${partSize}`);
      assert.equal(result.status, 400);
    }
  });

  it("rejects an upload that needs too many parts", () => {
    const result = planUpload({ sha256: SHA, size: 600, partSize: 10 }, limits, usage);
    assert.equal(result.ok, false);
    assert.equal(result.status, 400);
  });

  it("enforces the room quota across stored and in-flight uploads", () => {
    const tight = { completedBytes: 900, reservedBytes: 50 };
    const over = planUpload({ sha256: SHA, size: 60, partSize: 60 }, limits, tight);
    assert.deepEqual(over, { ok: false, status: 507, error: "Room media quota exceeded" });
    const fits = planUpload({ sha256: SHA, size: 50, partSize: 50 }, limits, tight);
    assert.equal(fits.ok, true);
  });
});

describe("parts", () => {
  const upload = { id: "u", sha256: SHA, size: 100, partSize: 40, partCount: 3, parts: {} };

  it("expects equal parts and a short last part", () => {
    assert.equal(expectedPartSize(upload, 1), 40);
    assert.equal(expectedPartSize(upload, 2), 40);
    assert.equal(expectedPartSize(upload, 3), 20);
    assert.equal(expectedPartSize(upload, 0), null);
    assert.equal(expectedPartSize(upload, 4), null);
  });

  it("checks a part's number and size", () => {
    assert.deepEqual(checkPart(upload, 1, 40), { ok: true });
    assert.deepEqual(checkPart(upload, 3, 20), { ok: true });
    assert.equal(checkPart(upload, 3, 40).ok, false);
    assert.equal(checkPart(upload, 9, 40).ok, false);
  });

  it("tracks which parts are still missing, in order, and ignores re-sent parts", () => {
    assert.deepEqual(missingParts(upload), [1, 2, 3]);
    const one = recordPart(upload, 2, { hash: "h2", size: 40, etag: "e2" });
    assert.deepEqual(missingParts(one), [1, 3]);
    const again = recordPart(one, 2, { hash: "h2", size: 40, etag: "e2b" });
    assert.deepEqual(missingParts(again), [1, 3]);
    assert.equal(again.parts["2"].etag, "e2b", "the latest upload of a part wins");
    const done = recordPart(recordPart(again, 1, { hash: "h1", size: 40, etag: "e1" }), 3, {
      hash: "h3",
      size: 20,
      etag: "e3",
    });
    assert.deepEqual(missingParts(done), []);
  });
});
