import type { RoomDO } from "./room";

export interface Env {
  ROOMS: DurableObjectNamespace<RoomDO>;
  /** Rooms one address may create per minute. */
  CREATE_LIMITER: RateLimit;
  /** Lookups of room codes that do not exist, per address per minute. */
  LOOKUP_LIMITER: RateLimit;
  /** Turnstile secret key (`wrangler secret put TURNSTILE_SECRET`). */
  TURNSTILE_SECRET?: string;
  /** Override for tests; defaults to Cloudflare's siteverify endpoint. */
  TURNSTILE_VERIFY_URL?: string;
  /** "1" lets room creation skip Turnstile. Local development only. */
  DEV_ALLOW_UNVERIFIED_CREATE?: string;
  /** Extra browser origins (comma separated) allowed besides the site's own. */
  ALLOWED_ORIGINS?: string;
  /** A socket that has not joined within this long is closed (default 15000). */
  PREJOIN_MS?: string;
  /** Test hook: "1" makes the first room deletion fail once (needs the dev bypass on). */
  FAIL_R2_DELETE_ONCE?: string;
  /** Room media, keyed room/<room id>/<sha256>. */
  MEDIA: R2Bucket;
  /** Longest a changed room waits before writing to storage (default 5000). */
  FLUSH_MS?: string;
  /** A joined socket silent this long is dropped (default 90000). */
  IDLE_SOCKET_MS?: string;
  /** An empty room is deleted this long after its last player leaves (default 60 min). */
  IDLE_CLEANUP_MS?: string;
  /** A room whose creator never connects is deleted after this long (default 15 min). */
  UNCLAIMED_MS?: string;
  /** Media a room may hold, stored plus in flight (default 1 GiB). */
  ROOM_MEDIA_QUOTA_BYTES?: string;
  /** Largest single media file (default: the room quota). */
  MAX_MEDIA_BYTES?: string;
  /** Multipart part size bounds; R2 needs 5 MiB minimum (defaults 5 and 64 MiB). */
  MEDIA_MIN_PART_BYTES?: string;
  MEDIA_MAX_PART_BYTES?: string;
}
