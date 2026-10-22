import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { originAllowed, verifyTurnstile } from "../src/security.ts";

describe("originAllowed", () => {
  const host = "rooms.pieceoff.example";
  it("allows requests with no Origin (native clients)", () => {
    assert.equal(originAllowed(null, host, ""), true);
  });
  it("allows the site's own origin", () => {
    assert.equal(originAllowed("https://rooms.pieceoff.example", host, ""), true);
    assert.equal(originAllowed("http://rooms.pieceoff.example", host, ""), true);
  });
  it("refuses other origins unless listed", () => {
    assert.equal(originAllowed("https://evil.example", host, ""), false);
    assert.equal(originAllowed("https://play.example", host, "https://play.example,https://x.example"), true);
    assert.equal(originAllowed("https://evil.example", host, "https://play.example"), false);
  });
  it("does not match on prefixes or substrings", () => {
    assert.equal(originAllowed("https://rooms.pieceoff.example.evil.test", host, ""), false);
    assert.equal(originAllowed("https://play.example.evil.test", host, "https://play.example"), false);
  });
  it("refuses a malformed Origin", () => {
    assert.equal(originAllowed("not a url", host, ""), false);
    assert.equal(originAllowed("null", host, ""), false);
  });
});

describe("verifyTurnstile", () => {
  const ok = (body) => async () => new Response(JSON.stringify(body), { status: 200 });
  const args = { secret: "s", token: "t", ip: "1.2.3.4", url: "https://verify.test/x" };

  it("accepts when the service says success", async () => {
    assert.equal(await verifyTurnstile({ ...args, fetcher: ok({ success: true }) }), true);
  });
  it("sends the secret, token and client address as a form", async () => {
    let seen;
    const fetcher = async (url, init) => {
      seen = { url, body: String(init.body), method: init.method };
      return new Response(JSON.stringify({ success: true }));
    };
    await verifyTurnstile({ ...args, fetcher });
    assert.equal(seen.method, "POST");
    assert.equal(seen.url, args.url);
    assert.match(seen.body, /secret=s/);
    assert.match(seen.body, /response=t/);
    assert.match(seen.body, /remoteip=1\.2\.3\.4/);
  });
  it("denies when the service says failure", async () => {
    assert.equal(await verifyTurnstile({ ...args, fetcher: ok({ success: false }) }), false);
  });
  it("fails closed on a missing token, bad status, bad JSON and network errors", async () => {
    assert.equal(await verifyTurnstile({ ...args, token: "", fetcher: ok({ success: true }) }), false);
    const status = async () => new Response("{}", { status: 500 });
    assert.equal(await verifyTurnstile({ ...args, fetcher: status }), false);
    const junk = async () => new Response("nope", { status: 200 });
    assert.equal(await verifyTurnstile({ ...args, fetcher: junk }), false);
    const down = async () => {
      throw new Error("unreachable");
    };
    assert.equal(await verifyTurnstile({ ...args, fetcher: down }), false);
    const truthy = ok({ success: "yes" });
    assert.equal(await verifyTurnstile({ ...args, fetcher: truthy }), false, "only literal true counts");
  });
});
