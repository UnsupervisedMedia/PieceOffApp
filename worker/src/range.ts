/** What to do with a `Range` header for an object of a known size. */
export type RangeDecision =
  | { kind: "full" }
  | { kind: "range"; start: number; end: number }
  | { kind: "unsatisfiable" };

/**
 * Parse a single `bytes=` range (RFC 9110). Headers we do not support (other
 * units, several ranges, malformed or backwards ranges) are ignored and the
 * whole object is served; a range that starts past the end, or an empty
 * suffix, is unsatisfiable. `end` is inclusive and clamped to the object.
 */
export function parseRange(header: string | null, size: number): RangeDecision {
  if (header === null) return { kind: "full" };
  const match = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
  if (match === null || (match[1] === "" && match[2] === "")) return { kind: "full" };
  const [, first, last] = match as unknown as [string, string, string];

  if (first === "") {
    const suffix = Number(last);
    if (!Number.isSafeInteger(suffix)) return { kind: "full" };
    if (suffix === 0 || size === 0) return { kind: "unsatisfiable" };
    return { kind: "range", start: Math.max(0, size - suffix), end: size - 1 };
  }
  const start = Number(first);
  if (!Number.isSafeInteger(start)) return { kind: "full" };
  if (last !== "") {
    const end = Number(last);
    if (!Number.isSafeInteger(end) || end < start) return { kind: "full" };
    if (start >= size) return { kind: "unsatisfiable" };
    return { kind: "range", start, end: Math.min(end, size - 1) };
  }
  if (start >= size) return { kind: "unsatisfiable" };
  return { kind: "range", start, end: size - 1 };
}
