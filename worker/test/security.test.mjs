import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { ipKey, originAllowed, trustedVerifyUrl, verifyTurnstile } from "../src/security.ts";

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

describe("ipKey", () => {
  it("leaves IPv4 addresses alone", () => {
    assert.equal(ipKey("203.0.113.9"), "203.0.113.9");
    assert.equal(ipKey("local"), "local");
  });
  it("groups IPv6 addresses by their /64", () => {
    assert.equal(ipKey("2001:db8:1:2:aaaa:bbbb:cccc:dddd"), "2001:db8:1:2::/64");
    assert.equal(ipKey("2001:db8:1:2:1111:2222:3333:4444"), "2001:db8:1:2::/64");
    assert.equal(ipKey("2001:DB8::1"), "2001:db8:0:0::/64");
    assert.equal(ipKey("::1"), "0:0:0:0::/64");
  });
});

describe("trustedVerifyUrl", () => {
  it("only honours loopback overrides so production keeps Cloudflare's endpoint", () => {
    assert.equal(trustedVerifyUrl("http://127.0.0.1:9/verify"), "http://127.0.0.1:9/verify");
    assert.equal(trustedVerifyUrl("http://localhost:9/verify"), "http://localhost:9/verify");
    assert.equal(trustedVerifyUrl("https://evil.example/verify"), undefined);
    assert.equal(trustedVerifyUrl("not a url"), undefined);
    assert.equal(trustedVerifyUrl(undefined), undefined);
  });
});
