import type { RoomDO } from "./room";

export interface Env {
  ROOMS: DurableObjectNamespace<RoomDO>;
  /** Room media, keyed room/<room id>/<sha256>. */
  MEDIA: R2Bucket;
  /** Longest a changed room waits before writing to storage (default 5000). */
  FLUSH_MS?: string;
  /** A joined socket silent this long is dropped (default 90000). */
  IDLE_SOCKET_MS?: string;
  /** Media a room may hold, stored plus in flight (default 1 GiB). */
  ROOM_MEDIA_QUOTA_BYTES?: string;
  /** Largest single media file (default: the room quota). */
  MAX_MEDIA_BYTES?: string;
  /** Multipart part size bounds; R2 needs 5 MiB minimum (defaults 5 and 64 MiB). */
  MEDIA_MIN_PART_BYTES?: string;
  MEDIA_MAX_PART_BYTES?: string;
}
