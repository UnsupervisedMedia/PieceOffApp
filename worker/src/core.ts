/**
 * The Rust room core, initialised once per isolate. Every module that needs
 * the core imports it from here so initialisation happens before first use.
 */
import { Room, builtin_media_title, initSync, protocol_version } from "../wasm/pieceoff_room_wasm.js";
import wasmModule from "../wasm/pieceoff_room_wasm_bg.wasm";

initSync({ module: wasmModule });

export { Room };

/** The wire protocol version the room core speaks. */
export const PROTOCOL_VERSION: number = protocol_version();

/** The bundled title of a built-in loop, or null for any other fingerprint. */
export function builtinTitle(sha256: string): string | null {
  return builtin_media_title(sha256) ?? null;
}

export const UPLOADS_OFF_ERROR =
  "PieceOff Online uses built-in puzzles for now. Use Local or LAN to play your own files.";

/** Hosted uploads are off unless `MEDIA_UPLOADS_ENABLED` is exactly "1". */
export function uploadsEnabled(env: { MEDIA_UPLOADS_ENABLED?: string }): boolean {
  return env.MEDIA_UPLOADS_ENABLED === "1";
}

/** Every media fingerprint a `create_room` message brings, including an imported snapshot's queue. */
export function createRoomMedia(message: Record<string, unknown>): string[] {
  const found: string[] = [];
  const add = (value: unknown) => {
    if (typeof value === "string" && value !== "") found.push(value);
  };
  add(message.media_sha256);
  const snapshot = message.initial_snapshot as { media_sha256?: unknown; queue?: unknown } | null | undefined;
  if (snapshot && typeof snapshot === "object") {
    add(snapshot.media_sha256);
    if (Array.isArray(snapshot.queue)) {
      for (const item of snapshot.queue) add((item as { sha256?: unknown } | null)?.sha256);
    }
  }
  return found;
}
