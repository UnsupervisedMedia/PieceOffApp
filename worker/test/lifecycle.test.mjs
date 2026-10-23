import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { cleanupDeadline } from "../src/lifecycle.ts";

const base = { joinedSockets: 0, emptySince: 1_000, idleMs: 60_000, unclaimed: false, unclaimedMs: 10_000 };

describe("cleanupDeadline", () => {
  it("never expires a room someone is in", () => {
    assert.equal(cleanupDeadline({ ...base, joinedSockets: 1 }), null);
  });

  it("expires an empty room one idle period after it emptied", () => {
    assert.equal(cleanupDeadline(base), 61_000);
  });

  it("uses the shorter unclaimed timeout while the creator has never connected", () => {
    assert.equal(cleanupDeadline({ ...base, unclaimed: true }), 11_000);
  });

  it("does not shorten the idle timeout when the unclaimed one is longer", () => {
    assert.equal(cleanupDeadline({ ...base, unclaimed: true, unclaimedMs: 120_000 }), 61_000);
  });

  it("has no deadline for a room that was never set up", () => {
    assert.equal(cleanupDeadline({ ...base, emptySince: null }), null);
  });
});
