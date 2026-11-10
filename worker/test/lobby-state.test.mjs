import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { entryChanged, expired, lobbyAction, lobbyCheckTime, normalizeEntry, sanitizeText, sortLobby } from "../src/lobby-state.ts";

const entry = (over = {}) => ({
  code: "ABCDEFGH", title: "Slow Orbit", host_name: "Host", players: 1, max_players: 8,
  spectators: 0, pieces: 12, placed: 0, cut_style: "jigsaw", created_at: 1000, updated_at: 1000, ...over,
});

describe("sanitizeText", () => {
  it("drops control and direction characters, collapses whitespace, caps length", () => {
    assert.equal(sanitizeText("  a‮b\u0000c\n\td  ", 24), "a b c d");
    assert.equal(sanitizeText("x".repeat(40), 24), "x".repeat(24));
    assert.equal(Array.from(sanitizeText("😀".repeat(30), 24)).length, 24);
    assert.equal(sanitizeText(42, 24), "");
  });
});

describe("normalizeEntry", () => {
  it("refuses bad codes and empty titles, clamps counts", () => {
    assert.equal(normalizeEntry(entry({ code: "bad" }), 0), null);
    assert.equal(normalizeEntry(entry({ title: "\u0000" }), 0), null);
    const tidy = normalizeEntry(entry({ placed: 99, cut_style: "<b>", host_name: "a⁦b" }), 0);
    assert.equal(tidy.placed, 12);
    assert.equal(tidy.cut_style, "jigsaw");
    assert.equal(tidy.host_name, "a b");
  });
  it("ignores updated_at when deciding whether an entry changed", () => {
    assert.equal(entryChanged(entry(), entry({ updated_at: 5 })), false);
    assert.equal(entryChanged(entry(), entry({ players: 2 })), true);
    assert.equal(entryChanged(undefined, entry()), true);
  });
});

describe("sortLobby", () => {
  it("puts rooms with a free slot first, then fuller, then newer, at most 100", () => {
    const full = entry({ code: "AAAAAAAA", players: 8 });
    const two = entry({ code: "BBBBBBBB", players: 2 });
    const oneOld = entry({ code: "CCCCCCCC", players: 1, created_at: 1 });
    const oneNew = entry({ code: "DDDDDDDD", players: 1, created_at: 9 });
    assert.deepEqual(sortLobby([full, oneOld, two, oneNew]).map((e) => e.code),
      ["BBBBBBBB", "DDDDDDDD", "CCCCCCCC", "AAAAAAAA"]);
    const many = Array.from({ length: 150 }, (_, i) => entry({ code: "ABCDEFGH", created_at: i }));
    assert.equal(sortLobby(many).length, 100);
  });
});

describe("lobby timing", () => {
  const base = { wanted: true, listed: true, dirty: false, now: 100_000, lastSentAt: 95_000, throttleMs: 10_000, heartbeatMs: 60_000 };
  it("throttles upserts and heartbeats, never delays a remove", () => {
    assert.equal(lobbyAction({ ...base, listed: false, lastSentAt: 0 }), "upsert");
    assert.equal(lobbyAction({ ...base, dirty: true }), "none");
    assert.equal(lobbyAction({ ...base, dirty: true, lastSentAt: 80_000 }), "upsert");
    assert.equal(lobbyAction({ ...base, lastSentAt: 50_000 }), "none");
    assert.equal(lobbyAction({ ...base, lastSentAt: 30_000 }), "upsert");
    assert.equal(lobbyAction({ ...base, wanted: false, lastSentAt: 99_999 }), "remove");
    assert.equal(lobbyAction({ ...base, wanted: false, listed: false }), "none");
  });
  it("looks again as soon as a throttled upsert may go out, not a whole period later", () => {
    const checked = (over) => {
      const input = { ...base, ...over };
      return lobbyCheckTime({ ...input, action: lobbyAction(input) });
    };
    // Relisted (or changed) 5 s after the last upsert: due at 105 000, not 110 000.
    assert.equal(checked({ listed: false }), 95_000);
    assert.equal(checked({ dirty: true }), 95_000);
    // Sent, removed, or nothing due: the next look is a period from now.
    assert.equal(checked({ listed: false, lastSentAt: 0 }), 100_000);
    assert.equal(checked({}), 100_000);
    assert.equal(checked({ wanted: false }), 100_000);
    assert.equal(checked({ wanted: false, listed: false }), 100_000);
  });
  it("expires entries with no heartbeat for the expiry period", () => {
    assert.equal(expired(0, 149_999, 150_000), false);
    assert.equal(expired(0, 150_000, 150_000), true);
  });
});
