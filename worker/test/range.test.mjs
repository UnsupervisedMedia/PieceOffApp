import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { parseRange } from "../src/range.ts";

describe("parseRange", () => {
  it("serves the whole object when there is no usable Range header", () => {
    for (const header of [null, "", "items=0-1", "bytes=abc", "bytes=5-2", "bytes=0-1,4-5", "bytes=-", "bytes=1-2-3"]) {
      assert.deepEqual(parseRange(header, 10), { kind: "full" }, String(header));
    }
  });

  it("parses a closed range and clamps its end to the object", () => {
    assert.deepEqual(parseRange("bytes=2-4", 10), { kind: "range", start: 2, end: 4 });
    assert.deepEqual(parseRange("bytes=5-999", 10), { kind: "range", start: 5, end: 9 });
  });

  it("parses an open-ended range", () => {
    assert.deepEqual(parseRange("bytes=7-", 10), { kind: "range", start: 7, end: 9 });
  });

  it("parses a suffix range, capped at the whole object", () => {
    assert.deepEqual(parseRange("bytes=-3", 10), { kind: "range", start: 7, end: 9 });
    assert.deepEqual(parseRange("bytes=-99", 10), { kind: "range", start: 0, end: 9 });
  });

  it("reports ranges that start past the end, and empty suffixes, as unsatisfiable", () => {
    assert.deepEqual(parseRange("bytes=10-12", 10), { kind: "unsatisfiable" });
    assert.deepEqual(parseRange("bytes=50-60", 10), { kind: "unsatisfiable" });
    assert.deepEqual(parseRange("bytes=-0", 10), { kind: "unsatisfiable" });
  });

  it("treats any range of an empty object as unsatisfiable", () => {
    assert.deepEqual(parseRange("bytes=0-0", 0), { kind: "unsatisfiable" });
  });
});
