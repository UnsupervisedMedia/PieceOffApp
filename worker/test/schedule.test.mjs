import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { alarmTarget, shouldSetAlarm } from "../src/schedule.ts";

const base = {
  now: 1_000_000,
  dirtySince: null,
  flushMs: 5_000,
  deadline: -1,
  joinedSockets: 0,
  openSockets: 0,
  cleanupAt: null,
  sweepMs: 20_000,
  minGapMs: 100,
  maxAheadMs: 7 * 24 * 3_600_000,
};

describe("alarmTarget", () => {
  it("sets nothing for an idle, clean, empty room", () => {
    assert.equal(alarmTarget(base), null);
  });

  it("flushes dirty state even with nobody connected", () => {
    assert.equal(alarmTarget({ ...base, dirtySince: 998_000 }), 1_003_000);
  });

  it("ignores the playback deadline when nobody is connected", () => {
    assert.equal(alarmTarget({ ...base, deadline: 1_000_500 }), null);
  });

  it("uses the playback deadline when it is earlier than the liveness sweep", () => {
    const target = alarmTarget({ ...base, joinedSockets: 2, openSockets: 2, deadline: 1_005_000 });
    assert.equal(target, 1_005_000);
  });

  it("sweeps for dead sockets while players are connected", () => {
    assert.equal(alarmTarget({ ...base, joinedSockets: 1, openSockets: 1 }), 1_020_000);
  });

  it("also sweeps when only unjoined sockets are open, but ignores playback", () => {
    const target = alarmTarget({ ...base, openSockets: 1, deadline: 1_005_000 });
    assert.equal(target, 1_020_000);
  });

  it("never schedules closer than the minimum gap", () => {
    const target = alarmTarget({ ...base, joinedSockets: 1, openSockets: 1, deadline: base.now });
    assert.equal(target, base.now + 100);
  });

  it("wakes for the cleanup deadline even when nobody is connected", () => {
    assert.equal(alarmTarget({ ...base, cleanupAt: 1_300_000 }), 1_300_000);
  });

  it("picks the earlier of cleanup and a pending flush", () => {
    const target = alarmTarget({ ...base, dirtySince: 998_000, cleanupAt: 1_300_000 });
    assert.equal(target, 1_003_000);
  });

  it("clamps a far-future deadline to the maximum lead", () => {
    const target = alarmTarget({
      ...base,
      joinedSockets: 1,
      openSockets: 1,
      sweepMs: Number.MAX_SAFE_INTEGER,
      deadline: 9e15,
    });
    assert.equal(target, base.now + base.maxAheadMs);
  });
});

describe("shouldSetAlarm", () => {
  it("sets when there is none, when it is past, or when the new target is earlier", () => {
    assert.equal(shouldSetAlarm(null, 10, 5), true);
    assert.equal(shouldSetAlarm(4, 10, 5), true);
    assert.equal(shouldSetAlarm(20, 10, 5), true);
  });
  it("keeps an existing alarm that is already due sooner", () => {
    assert.equal(shouldSetAlarm(10, 20, 5), false);
    assert.equal(shouldSetAlarm(10, 10, 5), false);
  });
});
