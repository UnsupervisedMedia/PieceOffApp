import type { LobbyDO } from "./lobby";
import type { RegistryDO } from "./registry";
import type { RoomDO } from "./room";

export interface Env {
  /** "0" pauses all hosted play (see index.ts); anything else, or unset, is on. */
  ONLINE_PLAY_ENABLED?: string;
  ROOMS: DurableObjectNamespace<RoomDO>;
  /** Device links, device keys and the global media budget; one instance, "registry". */
  REGISTRY: DurableObjectNamespace<RegistryDO>;
  /** The public lobby; one instance, "lobby". */
  LOBBY: DurableObjectNamespace<LobbyDO>;
  /** Device link requests per address per minute (also approvals and admin calls, keyed apart). */
  LINK_LIMITER: RateLimit;
  /** Lobby reads per address per minute (also link polls, keyed apart). */
  LOBBY_LIMITER: RateLimit;
  /** Where players approve a device link (`?c=CODE` is appended). */
  LINK_URL?: string;
  /** Invite link base the apps share (`?c=CODE`). */
  SHARE_URL?: string;
  /** The site's origin: the only browser origin that may approve links or read the lobby cross-origin. */
  SITE_ORIGIN?: string;
  /** Secret for the `/api/admin/*` routes; they are off without it. */
  ADMIN_TOKEN?: string;
  /** Media all rooms may hold together, stored plus in flight (default 8 GiB). */
  MEDIA_GLOBAL_QUOTA_BYTES?: string;
  /** Rooms the whole service may create per UTC day, device keys and browsers together (default 2000). */
  GLOBAL_ROOMS_PER_DAY?: string;
  /** Device link requests the whole service may accept per UTC day (default 500). */
  GLOBAL_LINKS_PER_DAY?: string;
  /** Test hook: milliseconds added to the daily caps' clock (needs the dev bypass on). */
  TEST_CLOCK_OFFSET_MS?: string;
  /** Rooms one device key may create per UTC day (default 30). */
  DEVICE_ROOMS_PER_DAY?: string;
  /** Media bytes one device key's rooms may upload per UTC day (default 2 GiB). */
  DEVICE_MEDIA_BYTES_PER_DAY?: string;
  /** How long a device link waits for approval and collection (default 600000). */
  LINK_TTL_MS?: string;
  /** A lobby entry with no heartbeat for this long is dropped (default 150000). */
  LOBBY_EXPIRY_MS?: string;
  /** Least time between a room's lobby updates (default 10000). */
  LOBBY_THROTTLE_MS?: string;
  /** How often a listed room with players re-announces itself (default 60000). */
  LOBBY_HEARTBEAT_MS?: string;
  /** Seconds the edge may reuse a lobby answer (default 5; 0 turns the cache off). */
  LOBBY_CACHE_SECONDS?: string;
  /** How often a room with media reports its true total to the registry (default 30 min). */
  BUDGET_RECONCILE_MS?: string;
  /** "1" allows media that is not a built-in loop (uploads to R2). Off by default. */
  MEDIA_UPLOADS_ENABLED?: string;
  /** Test hook: "1" makes the first upload abort skip its budget release (needs the dev bypass on). */
  FAIL_BUDGET_RELEASE_ONCE?: string;
  /** Rooms one address may create per minute. */
  CREATE_LIMITER: RateLimit;
  /** Lookups of room codes that do not exist, per address per minute. */
  LOOKUP_LIMITER: RateLimit;
  /** Turnstile secret key (`wrangler secret put TURNSTILE_SECRET`). */
  TURNSTILE_SECRET?: string;
  /** Exact hostname where the room-creation widget runs. */
  TURNSTILE_HOSTNAME?: string;
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
