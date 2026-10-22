/**
 * Pure rules for a room's resumable media uploads: planning (sizes, part
 * counts, quota) and part bookkeeping. No storage or R2 here, so it can be
 * tested without workerd; the Durable Object persists the results.
 */

export interface Limits {
  /** Total media a room may hold, stored plus in flight. */
  quotaBytes: number;
  /** Largest single file. */
  maxFileBytes: number;
  /** R2 requires every part except the last to be at least this large. */
  minPartBytes: number;
  maxPartBytes: number;
  /** R2 allows at most 10,000 parts. */
  maxParts: number;
}

export interface Usage {
  /** Bytes of finished media in the room. */
  completedBytes: number;
  /** Declared bytes of uploads started but not finished. */
  reservedBytes: number;
}

export interface PartRecord {
  hash: string;
  size: number;
  etag: string;
}

export interface Upload {
  id: string;
  sha256: string;
  size: number;
  partSize: number;
  partCount: number;
  /** Keyed by part number as a string (JSON-friendly). */
  parts: Record<string, PartRecord>;
}

export type Failure = { ok: false; status: number; error: string };
export type Planned = { ok: true; partCount: number } | Failure;

const SHA256_RE = /^[0-9a-fA-F]{64}$/;

export const isSha256 = (value: string): boolean => SHA256_RE.test(value);

const fail = (status: number, error: string): Failure => ({ ok: false, status, error });

export function planUpload(
  request: { sha256: string; size: number; partSize: number },
  limits: Limits,
  usage: Usage,
): Planned {
  const { sha256, size, partSize } = request;
  if (!SHA256_RE.test(sha256)) return fail(400, "Invalid media fingerprint");
  if (!Number.isSafeInteger(size) || size <= 0) return fail(400, "Media size must be positive");
  if (!Number.isSafeInteger(partSize) || partSize <= 0) return fail(400, "Bad part size");
  if (size > limits.maxFileBytes) return fail(413, "Media file is too large");
  const partCount = Math.ceil(size / partSize);
  if (partCount > 1 && (partSize < limits.minPartBytes || partSize > limits.maxPartBytes)) {
    return fail(400, "Part size is outside the allowed range");
  }
  if (partCount > limits.maxParts) return fail(400, "Too many parts; use a larger part size");
  if (usage.completedBytes + usage.reservedBytes + size > limits.quotaBytes) {
    return fail(507, "Room media quota exceeded");
  }
  return { ok: true, partCount };
}

/** Size part `n` (1-based) must have, or null if there is no such part. */
export function expectedPartSize(
  upload: Pick<Upload, "size" | "partSize" | "partCount">,
  n: number,
): number | null {
  if (!Number.isInteger(n) || n < 1 || n > upload.partCount) return null;
  return n < upload.partCount ? upload.partSize : upload.size - upload.partSize * (upload.partCount - 1);
}

export function checkPart(
  upload: Pick<Upload, "size" | "partSize" | "partCount">,
  n: number,
  size: number,
): { ok: true } | Failure {
  const expected = expectedPartSize(upload, n);
  if (expected === null) return fail(400, "No such part");
  if (size !== expected) return fail(400, `Part ${n} must be ${expected} bytes`);
  return { ok: true };
}

/** Record a finished part. Re-sending a part replaces the earlier one. */
export function recordPart(upload: Upload, n: number, part: PartRecord): Upload {
  return { ...upload, parts: { ...upload.parts, [String(n)]: part } };
}

/** Part numbers still to upload, ascending. */
export function missingParts(upload: Upload): number[] {
  const missing: number[] = [];
  for (let n = 1; n <= upload.partCount; n++) {
    if (!(String(n) in upload.parts)) missing.push(n);
  }
  return missing;
}
