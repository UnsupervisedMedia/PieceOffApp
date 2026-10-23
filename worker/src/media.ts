import type { Env } from "./env";
import { parseRange } from "./range";

const SHA256_RE = /^[0-9a-fA-F]{64}$/;
const UUID_RE = /^[0-9a-f-]{36}$/;
const MAX_JSON_BODY = 4096;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

function failure(reply: { status: number; error: string; missing?: number[] }): Response {
  return json({ error: reply.error, ...(reply.missing ? { missing: reply.missing } : {}) }, reply.status);
}

function bearer(request: Request): string {
  const header = request.headers.get("authorization") ?? "";
  return header.startsWith("Bearer ") ? header.slice(7).trim() : "";
}

function hex(buffer: ArrayBuffer): string {
  return Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Media routes for one room, below `/api/rooms/<CODE>/media`:
 *   POST   ""                          start or resume   {sha256,size,part_size}
 *   GET    "/<uploadId>"               which parts are missing
 *   PUT    "/<uploadId>/parts/<n>"     stream one part   (x-part-sha256 header)
 *   POST   "/<uploadId>/complete"      finish
 *   DELETE "/<uploadId>"               abort and free its quota
 *   GET    "/<sha256>"                 download, honours Range
 * Uploads authenticate with the host's upload token, downloads with any
 * player's resume token, both as `Authorization: Bearer`.
 */
export async function handleMedia(
  request: Request,
  env: Env,
  code: string,
  rest: string,
): Promise<Response> {
  const stub = env.ROOMS.get(env.ROOMS.idFromName(code));
  const token = bearer(request);
  const segments = rest.split("/").filter((s) => s.length > 0);

  if (segments.length === 0 && request.method === "POST") {
    const declared = Number(request.headers.get("content-length") ?? "0");
    if (declared > MAX_JSON_BODY) return json({ error: "Message too large" }, 413);
    let body: { sha256?: unknown; size?: unknown; part_size?: unknown };
    try {
      body = JSON.parse((await request.text()).slice(0, MAX_JSON_BODY));
    } catch {
      return json({ error: "Bad message" }, 400);
    }
    const result = await stub.startUpload(token, {
      sha256: String(body.sha256 ?? ""),
      size: Number(body.size),
      partSize: Number(body.part_size),
    });
    if (!result.ok) return failure(result);
    if (result.complete) return json({ complete: true });
    return json({
      complete: false,
      upload_id: result.uploadId,
      part_count: result.partCount,
      part_size: result.partSize,
      missing: result.missing,
    });
  }

  if (segments.length === 1 && SHA256_RE.test(segments[0]!) && request.method === "GET") {
    return download(request, env, stub, token, segments[0]!.toLowerCase());
  }

  const uploadId = segments[0] ?? "";
  if (!UUID_RE.test(uploadId)) return json({ error: "Not found" }, 404);

  if (segments.length === 1 && request.method === "GET") {
    const status = await stub.uploadStatus(token, uploadId);
    if (!status.ok) return failure(status);
    return json({ part_count: status.partCount, part_size: status.partSize, missing: status.missing });
  }

  if (segments.length === 1 && request.method === "DELETE") {
    const aborted = await stub.abortUpload(token, uploadId);
    return aborted.ok ? json({ ok: true }) : failure(aborted);
  }

  if (segments.length === 2 && segments[1] === "complete" && request.method === "POST") {
    const done = await stub.completeUpload(token, uploadId);
    return done.ok ? json({ ok: true }) : failure(done);
  }

  if (segments.length === 3 && segments[1] === "parts" && request.method === "PUT") {
    return putPart(request, env, stub, token, uploadId, Number(segments[2]));
  }

  return json({ error: "Not found" }, 404);
}

async function putPart(
  request: Request,
  env: Env,
  stub: DurableObjectStub<import("./room").RoomDO>,
  token: string,
  uploadId: string,
  n: number,
): Promise<Response> {
  const size = Number(request.headers.get("content-length"));
  const claimed = (request.headers.get("x-part-sha256") ?? "").toLowerCase();
  if (!Number.isSafeInteger(size) || size <= 0 || request.body === null) {
    return json({ error: "A part needs a Content-Length and a body" }, 400);
  }
  if (!SHA256_RE.test(claimed)) return json({ error: "Missing x-part-sha256" }, 400);
  const target = await stub.preparePart(token, uploadId, n, size);
  if (!target.ok) return failure(target);

  // One pass over the body: each chunk is hashed and then handed on to R2 as a
  // fixed-length stream, so back-pressure reaches the client and a part is
  // never held in memory. R2 stores the bytes before the hash is compared, so
  // a mismatch leaves a bad part there; the Durable Object has already voided
  // its record of this part and a re-send replaces it.
  const digest = new crypto.DigestStream("SHA-256");
  const digestWriter = digest.getWriter();
  const fixed = new FixedLengthStream(size);
  const hashAndForward = new TransformStream<Uint8Array, Uint8Array>({
    async transform(chunk, controller) {
      await digestWriter.write(chunk);
      controller.enqueue(chunk);
    },
    async flush() {
      await digestWriter.close();
    },
  });
  const piping = request.body.pipeThrough(hashAndForward).pipeTo(fixed.writable);
  piping.catch(() => undefined); // surfaced through uploadPart; avoid an unhandled rejection
  let recorded = false;
  try {
    const stored = await env.MEDIA.resumeMultipartUpload(target.key, target.r2UploadId).uploadPart(
      n,
      fixed.readable,
    );
    await piping;
    const actual = hex(await digest.digest);
    if (actual !== claimed) return json({ error: "Part hash mismatch" }, 400);
    const result = await stub.recordUploadedPart(token, uploadId, n, {
      hash: actual,
      size,
      etag: stored.etag,
    });
    recorded = result.ok;
    return result.ok ? json({ missing: result.missing }) : failure(result);
  } catch (error) {
    console.error("part upload failed", String(error));
    return json({ error: "Part upload failed" }, 502);
  } finally {
    if (!recorded) await stub.releasePart(token, uploadId, n).catch(() => undefined);
  }
}

async function download(
  request: Request,
  env: Env,
  stub: DurableObjectStub<import("./room").RoomDO>,
  token: string,
  sha256: string,
): Promise<Response> {
  const target = await stub.authorizeDownload(token, sha256);
  if (!target.ok) return failure(target);
  const size = target.size;
  const decision = parseRange(request.headers.get("range"), size);
  const headers = new Headers({
    "content-type": "application/octet-stream",
    "accept-ranges": "bytes",
    "cache-control": "private, no-store",
  });
  if (decision.kind === "unsatisfiable") {
    headers.set("content-range", `bytes */${size}`);
    return new Response(null, { status: 416, headers });
  }
  const object =
    decision.kind === "range"
      ? await env.MEDIA.get(target.key, {
          range: { offset: decision.start, length: decision.end - decision.start + 1 },
        })
      : await env.MEDIA.get(target.key);
  if (object === null) return json({ error: "No such media" }, 404);
  headers.set("etag", object.httpEtag);
  let status = 200;
  let length = size;
  if (decision.kind === "range") {
    length = decision.end - decision.start + 1;
    headers.set("content-range", `bytes ${decision.start}-${decision.end}/${size}`);
    status = 206;
  }
  headers.set("content-length", String(length));
  return new Response(object.body, { status, headers });
}
