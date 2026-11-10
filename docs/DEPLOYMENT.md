# Deploying the room server

This guide is for running your own PieceOff room server. Self-hosting is
permitted under Apache-2.0, including for commercial use; there is no license
fee or royalty to PieceOff for doing so. The project may separately offer
hosted rooms at `pieceoff.app`, subject to separate service terms and
availability. Hosted service plans do not remove the right to run this server
yourself.

`pieceoff-server` is one process: WebSocket rooms plus an HTTP endpoint for
media. It keeps rooms and uploaded media on local disk, so it runs on a single
machine (no clustering; see [Limits](#limits)). Players need only its address.

There is no account system. A room is protected by its invite code (8
characters, about 40 bits) and media by the same code or the host's upload
token. Treat the server as a place friends meet, not as a public service with
moderation beyond the host's tools (remove a player, switch chat off): see
[Security notes](#security-notes).

## Docker with automatic HTTPS (recommended)

You need a machine with Docker, ports 80 and 443 open, and a DNS name pointing
at it.

```sh
cd deploy
cp .env.example .env            # set PIECEOFF_DOMAIN=rooms.example.com
docker compose up -d --build
```

This builds the server image (about 10 MiB of binary on a distroless base) and
starts it behind Caddy, which obtains and renews a certificate. The room server
is not published; only Caddy is. Players set **Server** to
`wss://rooms.example.com/ws` in the app. Check it:

```sh
docker compose ps                                   # pieceoff should be "healthy"
curl https://rooms.example.com/health               # PieceOff room server is running
docker compose logs pieceoff | head -20             # the settings it started with
```

Without compose: `docker build -t pieceoff-server .` then
`docker run -d -p 28878:28878 -v pieceoff-data:/data pieceoff-server`, and put
your own TLS terminator in front (the image listens on plain HTTP).

## systemd (no Docker)

Build `target/release/pieceoff-server` (`cargo build --release -p
pieceoff-server`; the server needs no GUI or audio libraries) and follow the
commands at the top of [`deploy/pieceoff-server.service`](../deploy/pieceoff-server.service).
It runs as an unprivileged `pieceoff` user with a locked-down sandbox, stores
data in `/var/lib/pieceoff`, and reads settings from `/etc/pieceoff/server.env`
([example](../deploy/pieceoff-server.env.example)). Bind to `127.0.0.1:28878`
and put Caddy or nginx on the same machine in front
([`Caddyfile`](../deploy/Caddyfile), [`nginx.conf.example`](../deploy/nginx.conf.example)).

## Reverse proxy requirements

Whatever you use must:

- **Pass WebSocket upgrades** on `/ws`.
- **Stream request bodies** without a size cap of their own: media is uploaded
  with `PUT /media/<sha256>` and can be many gigabytes. The server enforces
  `PIECEOFF_MAX_MEDIA_BYTES` itself, so let it do the refusing. (nginx:
  `client_max_body_size 0; proxy_request_buffering off;`.) Downloads honour
  `Range` requests so a dropped connection resumes; do not strip that header.
- **Allow long, quiet connections.** The server pings every 20 s and drops a
  socket silent for 60 s; keep proxy read timeouts above that (an hour is fine).
- **Overwrite `X-Forwarded-For`** with the real client address, and only then set
  `PIECEOFF_TRUST_PROXY=1`. If the server is reachable without the proxy, or the
  proxy passes the client's own header through, a client could pick its own
  address and dodge the rate limits. Leave it off when there is no proxy.

## Settings

All settings are environment variables. A value that cannot be read stops the
server with a message naming the variable.

| Variable | Default | Meaning |
| --- | --- | --- |
| `PIECEOFF_BIND` | `0.0.0.0:28878` | Address and port to listen on. |
| `PIECEOFF_DATA_DIR` | `pieceoff-data` (`/data` in the image) | Room snapshots and `media/`. |
| `PIECEOFF_MEDIA_QUOTA_BYTES` | 50 GiB | Total stored media; uploads beyond it get HTTP 507. Space is reserved when an upload starts, so concurrent uploads cannot together pass it. |
| `PIECEOFF_MAX_MEDIA_BYTES` | 16 GiB | Largest single media file; a larger declared upload gets HTTP 413 before any bytes are read. Memory use does not grow with file size. |
| `PIECEOFF_ROOM_TTL_DAYS` | 90 | Days an empty room is kept before it and its unshared media are deleted. `0` keeps rooms forever. |
| `PIECEOFF_MAX_ROOMS` | 500 | Rooms held at once; creating more is refused. |
| `PIECEOFF_MAX_CONNECTIONS` | 400 | Open sockets; more get HTTP 503. |
| `PIECEOFF_MAX_MESSAGE_BYTES` | 1048576 | Largest message accepted on a socket (minimum 262144, enough for a 400-piece puzzle). |
| `PIECEOFF_CONNECTIONS_PER_MINUTE` | 60 | New sockets per client address; more get HTTP 429. |
| `PIECEOFF_FAILED_JOINS_PER_HOUR` | 20 | Joins with an unknown room code per address before further joins are refused. This is the guess budget for invite codes. |
| `PIECEOFF_TRUST_PROXY` | `0` | Take the client address from `X-Forwarded-For` (see above). |
| `PIECEOFF_ALLOWED_ORIGINS` | none | Comma-separated browser origins (`https://play.example.com`) allowed to open a socket. Native clients send no origin and are always allowed; any other browser origin gets HTTP 403. |

The server prints these at startup. `pieceoff-server --healthcheck` asks a
running server (at `PIECEOFF_BIND`) whether it is up and exits 0 or 1; the
Docker image uses it, so no `curl` is needed inside the container.

## Data, backups, and upgrades

`PIECEOFF_DATA_DIR` holds one `<CODE>-<revision>-<time>.json` per room (the
latest only; older ones are removed) and `media/<sha256>.<ext>` for uploads.
Snapshots are written to a temporary file and renamed, so copying the
directory while the server runs gives a usable backup: restore it, start the
server, and rooms reappear with their media. Back up the whole directory
together; a snapshot without its media file shows as "media only on host".

Stopping the server (`docker compose stop`, `systemctl stop`, Ctrl-C) sends
SIGTERM, and the server writes every room out before exiting. Moves are saved
as players drop pieces, so even a crash loses at most a piece mid-drag.
Upgrade by replacing the binary or rebuilding the image; room files from older
protocol versions (2 and up) are migrated on load, and connected players
reconnect automatically.

Disk use grows with uploads, up to the media quota. Rooms nobody has joined
for `PIECEOFF_ROOM_TTL_DAYS` are deleted hourly together with media no other
room uses.

## Monitoring

- `GET /health` returns `200` and a fixed line while the process is serving.
- Logs go to stdout (`docker compose logs`, `journalctl -u pieceoff-server`):
  startup settings, players joining, restored and expired rooms, rejected
  uploads.
- Watch free disk space on the data volume; a full disk fails uploads and
  room saves.

## Security notes

- **Invite codes** are 8 characters from a 32-character alphabet. Guessing
  is bounded by `PIECEOFF_FAILED_JOINS_PER_HOUR` per address, but anyone with a
  code can join, view the media, and move pieces: share codes like links to a
  private document.
- **Media quota and per-file cap** limit disk use, but any room creator can
  fill the quota with uploads. If strangers can reach the server, lower
  `PIECEOFF_MAX_ROOMS` and the quota, or keep it on a private network or behind
  a VPN. The server does not inspect uploaded media; it is stored and served
  as opaque files by content hash.
- **Browser origins** are refused unless listed, so a web page cannot use a
  visitor's browser to reach a server on their private network.
- **No TLS in the server.** Always terminate TLS in front of an internet-facing
  server: without it, invite codes and media cross the network in the clear.
- Run the image or unit as the provided unprivileged user. Do not expose
  `28878` directly when a proxy is in use.

## Cloudflare: PieceOff Online (`rooms.pieceoff.app`)

The hosted service runs on Cloudflare Workers, Durable Objects and R2; the
axum server above is unchanged. The room rules run in the Durable Object as the
`pieceoff-room-wasm` module. Worker `pieceoff-rooms` serves the custom domain
`rooms.pieceoff.app`. It was created once by hand; every later deploy comes
from Git through Cloudflare Workers Builds. **Never run `wrangler deploy` or
any direct upload by hand.**

Worker size gate, measured with `scripts/wasm-size.sh` on 2026-10-21 (no
`wasm-opt`, no brotli installed): **288,099 bytes raw, 96,851 bytes gzip**.
The free-plan Worker limit is 3 MB compressed, so the room core uses about 3%
of it.

### Workers Builds settings

| Setting | Value |
| --- | --- |
| Repository | `bourneash/PieceOff` (private deploy mirror), production branch `deploy` |
| Root directory | `/worker` |
| Build command | `./ci-build.sh` |
| Deploy command | `npx wrangler deploy` (Workers Builds runs it; nobody else does) |
| Non-production branch deploys | off |

The build image has Node but no Rust. `worker/ci-build.sh` installs what is
missing (rustup with the minimal profile after checking `rustup-init` against
its published sha256, the `wasm32-unknown-unknown` target, and the prebuilt
`wasm-bindgen` 0.2.129 release, whose sha256 is pinned in the script), then
runs `worker/build.sh`. Workers Builds installs the npm dependencies from
`worker/package-lock.json` itself. A cold build spends about a minute
installing Rust; the wasm build is under a minute.

The Cloudflare GitHub app is connected to the `bourneash` account, which cannot
see repositories owned by `UnsupervisedMedia`, so Workers Builds reads a
private mirror. `.github/workflows/mirror.yml` pushes every commit on `main` to
`bourneash/PieceOff` branch `deploy` (a fast-forward; `main` is linear) using
the `MIRROR_DEPLOY_KEY` secret, an SSH deploy key with write access to that
mirror only. Without the secret the workflow warns and does nothing, and the
Worker stays on its last build. The mirror's own `main` is an unrelated older
copy; leave it alone.

### Emergency pause

Set the Worker **secret** `ONLINE_PLAY_ENABLED` to `0` (dashboard, or the
secrets API) to stop all new hosted play at once: `/api/lobby` answers an empty
list and linking, creating, joining and media answer 503 "paused", all without
waking a Durable Object. `/health`, `/api/info` and `/api/admin/*` keep
working. A secret, unlike a `vars` entry, survives the next Git deploy. Delete
the secret (or set it to `1`) to resume.

### Daily ceilings

The account is a Workers Paid plan with no provider spend cap, shared with
other sites, so the Worker stops itself before many devices and addresses
together can run up the bill. On top of the per-address rate limits and the
per-device caps, the registry keeps two service-wide counters per UTC day:

- `GLOBAL_ROOMS_PER_DAY` (default 2000): every room creation, device-key and
  browser (Turnstile) alike. A creation that is admitted but then fails (bad
  `create_room`, no free code) gives its slot back, as it does the device's.
- `GLOBAL_LINKS_PER_DAY` (default 500): `POST /api/links`.

Past either ceiling the call answers
`503 {"error":"PieceOff Online has reached today's limit. Please try again tomorrow."}`
until midnight UTC. Refused requests (rate limited, failed Turnstile, bad or
capped device key) never take a slot. Joining existing rooms, media, the lobby,
link approvals and polls keep working. Each counted action costs one SQLite row
write in the registry and nothing more. `GET /api/admin/usage` (below) reports
today's counts so the fleet can poll them; the emergency pause above is the
switch for stopping everything at once.

### Settings

Plain variables live in `worker/wrangler.jsonc` (`vars`), so a deploy from Git
always carries them; change them there, not in the dashboard.

| Variable | Default | Meaning |
| --- | --- | --- |
| `MEDIA_UPLOADS_ENABLED` | `0` | Anything but `1` refuses every media item that is not a built-in loop, in every room (see below). |
| `SITE_ORIGIN` | `https://pieceoff.app` | The only browser origin that may approve device links and read cross-origin responses (`Access-Control-Allow-Origin`). Never `*`. |
| `TURNSTILE_HOSTNAME` | `pieceoff.app` | Hostname the Turnstile widget runs on (link approvals, browser room creation). |
| `LINK_URL` / `SHARE_URL` | `https://pieceoff.app/link` / `https://pieceoff.app/join` | Reported by `/api/info`; the link URL gets `?c=CODE`. |
| `MEDIA_GLOBAL_QUOTA_BYTES` | 8 GiB | Media all rooms together may hold, stored plus in flight. A hard stop under R2's 10 GB free tier. |
| `GLOBAL_ROOMS_PER_DAY` | 2000 | Rooms the whole service may create per UTC day (see [Daily ceilings](#daily-ceilings)). |
| `GLOBAL_LINKS_PER_DAY` | 500 | Device link requests the whole service may accept per UTC day. |
| `DEVICE_ROOMS_PER_DAY` | 30 | Rooms one device key may create per UTC day. |
| `DEVICE_MEDIA_BYTES_PER_DAY` | 2 GiB | Media bytes one device's rooms may start uploading per UTC day. |
| `ROOM_MEDIA_QUOTA_BYTES` | 1 GiB | Per room, stored plus in flight. `MAX_MEDIA_BYTES`, `MEDIA_MIN_PART_BYTES`, `MEDIA_MAX_PART_BYTES` bound files and parts. |
| `FLUSH_MS`, `IDLE_SOCKET_MS`, `PREJOIN_MS`, `IDLE_CLEANUP_MS`, `UNCLAIMED_MS` | 5000, 90000, 15000, 3600000, 900000 | Write-behind delay, silent-socket drop, join deadline, empty-room expiry, never-claimed-room expiry. |
| `ALLOWED_ORIGINS` | none | Extra browser origins (comma separated) for sockets, media and room creation. |
| `LINK_TTL_MS`, `LOBBY_EXPIRY_MS`, `LOBBY_THROTTLE_MS`, `LOBBY_HEARTBEAT_MS`, `LOBBY_CACHE_SECONDS`, `BUDGET_RECONCILE_MS` | 600000, 150000, 10000, 60000, 5, 1800000 | Timing knobs, normally left alone (the tests shorten them). |

Secrets (dashboard → Worker → Settings → Variables and Secrets, type
*Secret*; never committed):

| Secret | Needed for |
| --- | --- |
| `TURNSTILE_SECRET` | Link approvals and browser room creation. Without it both are refused, so no device can be linked. The live Worker has none yet. |
| `ADMIN_TOKEN` | `POST /api/admin/devices/revoke` and `GET /api/admin/usage`. Without it both answer 404. Use a long random value. |

Test-only, never set in production: `DEV_ALLOW_UNVERIFIED_CREATE=1` (skips
Turnstile), `TURNSTILE_VERIFY_URL` (honoured only for loopback),
`FAIL_R2_DELETE_ONCE`, `FAIL_BUDGET_RELEASE_ONCE`, `TEST_CLOCK_OFFSET_MS`
(moves the daily ceilings' clock; honoured only with the dev bypass on).

Bindings: Durable Objects `ROOMS` (`RoomDO`, one per room), `REGISTRY`
(`RegistryDO`, one instance named `registry`), `LOBBY` (`LobbyDO`, one
instance named `lobby`), all SQLite-backed (migrations `v1`, `v2`); R2 `MEDIA`
(bucket `pieceoff-media`); rate limits `CREATE_LIMITER` (10 rooms per minute
per address), `LOOKUP_LIMITER` (20 room-code socket requests per minute per
address), `LINK_LIMITER` (10 per minute per address, separately for link
requests, approvals and admin calls), `LOBBY_LIMITER` (60 per minute per
address, separately for lobby reads and link polls).

### Hosted uploads are off at launch

With `MEDIA_UPLOADS_ENABLED` not `1`, the Worker accepts only the built-in
loops: `create_room` with any other `media_sha256` or imported queue item gets
403, `queue_add` of another item gets an `error` frame, and starting a media
upload gets 403, all with "PieceOff Online uses built-in puzzles for now. Use
Local or LAN to play your own files." `/api/info` reports `"uploads": false`
so the app can grey out its own files on Online. Built-in items never touch
R2 either way. **Turn uploads on only once an abuse contact, a takedown
process and a known-bad hash blocklist exist.** The upload, budget and
per-device byte code stays tested with the flag on.

### Worker endpoints

All JSON with `cache-control: no-store` unless noted. Browser calls are
refused (403) unless their `Origin` is the Worker's own host or listed in
`ALLOWED_ORIGINS`; native apps send no `Origin`.

| Call | Purpose |
| --- | --- |
| `GET /health` | Plain-text liveness line. |
| `GET /api/info` | `{"service":"pieceoff-rooms","protocol":15,"hosted":true,"link_url":…,"share_url":…,"features":["device_keys","lobby"],"uploads":false}`. The axum server answers 404, which is how the app tells them apart. |
| `POST /api/links` | Body optional `{"device_name": "…"}` (tidied, at most 40 characters). → `{"link_code":"ABCD2345","poll_secret":"<43 b64url>","url":"<link_url>?c=ABCD2345","expires_in":600}`. 429 over `LINK_LIMITER`; 503 past `GLOBAL_LINKS_PER_DAY`. |
| `OPTIONS /api/links/<code>/approve` | Preflight: 204, `Access-Control-Allow-Origin: <SITE_ORIGIN>`, `-Methods: POST`, `-Headers: x-turnstile-token, content-type`. |
| `POST /api/links/<code>/approve` | Browser only: `Origin` must be `SITE_ORIGIN`, header `x-turnstile-token` (action `link_device`, hostname `TURNSTILE_HOSTNAME`). → `{"ok":true}`; 404 unknown or expired; 409 already approved; 403 bad origin or token. Every answer carries `Access-Control-Allow-Origin`. |
| `GET /api/links/<code>` | `Authorization: Bearer <poll_secret>`. → `202 {"status":"pending"}`, or `200 {"status":"approved","device_key":"pok_<43 b64url>"}` exactly once (the link is then deleted), or 404 (unknown, expired, wrong secret). Poll every 2 s. |
| `POST /api/rooms` | A `create_room` body. Either `Authorization: Bearer pok_…` (device key, no Turnstile) or `x-turnstile-token` (browser, action `create_room`). → `{code, client_id, resume_token}`. Bad or revoked key → `401 {"error":…,"relink":true}`; over the device's daily rooms → 429; past `GLOBAL_ROOMS_PER_DAY` → 503. |
| `GET /api/lobby` | `{"rooms":[{code,title,host_name,players,max_players,spectators,pieces,placed,cut_style,created_at,updated_at}]}`, at most 100, rooms with a free player slot first, then more players, then newest. `cache-control: public, max-age=5`, `Access-Control-Allow-Origin: <SITE_ORIGIN>`, and cached for 5 s at the edge. 429 over `LOBBY_LIMITER`. |
| `POST /api/admin/devices/revoke` | `Authorization: Bearer <ADMIN_TOKEN>` (constant-time compare). Body `{"device_key":"pok_…"}` or `{"device_prefix":"<16 hex>"}`, optional `"revoked": false` to restore. → `{"ok":true,"revoked":true,"devices":1}`; 404 no such device. |
| `GET /api/admin/usage` | `Authorization: Bearer <ADMIN_TOKEN>` (constant-time compare; rate limited with the other admin calls). → `{"day":"2026-11-10","rooms":12,"rooms_cap":2000,"links":3,"links_cap":500}`, today's UTC counts against the daily ceilings. 403 wrong token; 404 without `ADMIN_TOKEN` set. Works while paused. |

Rooms: clients then open `/ws?code=<CODE>` and send `join_room` first. A
player's `client_id` is public (everyone sees the host's id), so it proves
nothing; to come back as the same player, including the creator claiming the
host role, put the `resume_token` in the `join_room` frame. Every `welcome`
carries the joiner's own token; the server stores only its hash, and keeps the
newest 64 for players who have left. Protocol v15 spectators, name claims,
room cursors, chat, board pings, timelapse requests, and the public-listing
rule (`listed` rooms use built-in media only; built-in items never upload to
R2) use the shared Rust room rules, as does moderation: the host's
`{"type":"kick_player","name":"…"}` sends the removed connection(s)
`{"type":"kicked","client_id":"…","message":"The host removed you from this room."}`,
closes them (1008), deletes their resume tokens, and bans the name and player
key for the room's life (refused joins get an `error` with the same message);
`{"type":"set_chat_enabled","enabled":bool}` switches chat (listed rooms start
with it off; chat is then refused with "The host turned chat off in this
room." and newcomers get no `chat_history`, pings still work); and names
joining a listed room pass the core's short blocklist ("Pick a different name
for public rooms"). Name claims, bans (at most 64) and timelapse are saved with
the room, flushed at once after a kick; chat and cursors are ephemeral. Clients send the text `ping` about
every 20 seconds and get `pong`.

Media (`/api/rooms/<CODE>/media...`, bearer tokens in `Authorization`):

| Call | Auth | Purpose |
| --- | --- | --- |
| `POST /media` `{sha256,size,part_size}` | host upload token | Start or resume; returns `upload_id`, `part_count`, `missing` |
| `GET /media/<upload_id>` | upload token | Which parts are missing |
| `PUT /media/<upload_id>/parts/<n>` | upload token | Stream one part; `x-part-sha256` and `Content-Length` required, hash verified while streaming |
| `POST /media/<upload_id>/complete` | upload token | Finish; refused with the missing parts if incomplete |
| `DELETE /media/<upload_id>` | upload token | Abort: frees the R2 parts and the reserved quota |
| `GET /media/<sha256>` | any player's resume token | Download, honours `Range` |

Only media already waiting in the room's queue (a `queue_add` item that is not
yet available) can be uploaded. One send per part at a time: a second
simultaneous send of the same part gets 409, and sending a part again voids the
earlier copy until the new one is verified, so a rejected re-send simply shows
the part as missing again. Downloads parse a single `bytes=` range; anything
unusable is ignored (full body) and an unsatisfiable range gets 416.

Objects live at `room/<room id>/<sha256>` in R2. All parts but the last must be
the declared part size (R2 needs 5 MiB minimum, up to 10,000 parts). The upload
token goes only to the host over the socket. The full-file hash is not checked
by the worker; clients verify it after download.

### Device keys

A desktop app links itself once: it asks for a link code, opens
`pieceoff.app/link?c=CODE` where the player passes Turnstile, and polls until
it collects its device key (`pok_…`). The registry stores only SHA-256 hashes
of poll secrets and keys; a key is minted at the moment the app collects it,
so its plain value is never stored. Each key has created and last-used times,
daily room and media-byte counters (reset at 00:00 UTC), totals, and a
revoked flag. A room made with a key records the first 16 hex characters of
the key's hash (`creator_device`); revoke a key with that prefix:

```sh
curl -X POST https://rooms.pieceoff.app/api/admin/devices/revoke \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H 'content-type: application/json' \
  -d '{"device_prefix":"0123456789abcdef"}'
```

A revoked or unknown key gets `401 … "relink": true` and the app links again.

### Global media budget

`RegistryDO` keeps each room's media bytes (stored plus reserved) in its own
row, against `MEDIA_GLOBAL_QUOTA_BYTES`. A room reserves the declared size
before a multipart upload starts (also charging its creator device's day) and
gives it back when the upload is aborted or fails to start, and its row is
dropped when the room expires and its R2 objects are deleted. Over the
budget: `507 {"error":"PieceOff Online is out of media space right now. Try a
smaller file or try later."}` (a device over its daily bytes also gets 507).
Crash safety: a room with media reports its true total from its alarm at most
every `BUDGET_RECONCILE_MS`, which replaces whatever a lost release or a crash
left behind; rows no room has reported for `IDLE_CLEANUP_MS` plus a day are
dropped. A room's budget calls run one at a time so a report never races a
reservation. A completion that R2 rejects keeps its reservation while the
upload can still be retried or aborted.

### Lobby

A listed room (built-in loops only) with at least one player sends its entry
to `LobbyDO`: when something visible changes (players, spectators, placed
pieces, current item, listing), at most once per 10 s, and at least every
60 s while players are in. It is removed as soon as the room is delisted,
has no players, or expires, and within 10 s of being completed; an entry with
no heartbeat for 150 s is dropped. The title is the bundled title of the
current built-in loop, never client text; `host_name` is the host's display
name with control and direction characters removed, at most 24 characters,
and blank when the public name filter refuses it.
Heartbeats only touch memory; the lobby writes a row only when an entry's
content changes. The minimum moderation is in place (host kick with a rejoin
ban, local mute, chat off by default in listed rooms, the name filter, and
**More → Report room…**, a `mailto:contact@pieceoff.app` link with the code).
Reports land in that mailbox only; nothing on the service acts on them, and a
removed player who clears their settings and picks a new name gets back in
(no accounts, no address bans). Decide whether that is enough before
announcing the lobby (`ONLINE_PLAY_LIVE`).

### Cost on the free plan

Checked against Cloudflare's pricing pages on 2026-10-21: Durable Object
requests count HTTP requests, RPC calls, WebSocket messages (20 : 1) and alarm
invocations; each `setAlarm()` is a row written; the free plan allows 100,000
Worker requests, 100,000 Durable Object requests, 13,000 GB-s, and 100,000
SQLite rows written per day, and only SQLite-backed Durable Objects. R2 is
free to 10 GB with free egress, hence the 8 GiB global budget. What the new
parts cost:

- A device link: a few registry requests and rows (create, approve, collect),
  plus one request per 2 s poll while waiting.
- Device-key room creation: one registry request and one row.
- Uploads (when enabled): one registry request per start and per abort, and
  one reconcile per room with media every 30 minutes while it is in use.
- Lobby: per listed room with players, at most one request every 10 s while
  it changes and one per minute otherwise; rows only on visible changes. The
  lobby's own alarm runs at most every 150 s while it has entries. Reads hit
  the lobby object at most once per 5 s per edge location; every read still
  counts as a Worker request, so heavy lobby polling is the first thing to
  watch.

### Local testing

`scripts/worker-test.sh` builds the wasm module (`worker/build.sh`), type-checks
`worker/`, and runs the unit and integration tests against a local
`wrangler dev --local` (identity, piece leases, restart recovery, silent-socket
sweep, multipart upload and resume, ranged download, quota, room expiry and R2
cleanup, Turnstile, rate limits, origins, socket caps, `/api/info`, device
link create/approve/poll and one-time key delivery, CORS, device-key rooms
with the daily cap and 401 relink for unknown and revoked keys, the global
budget's 507, release on abort and expiry and reconcile after a lost release,
lobby listing, updates, removal and expiry with built-in titles, the
listed-room built-in rule, the uploads-off refusals, the daily ceilings (503 for links and for
device-key and browser rooms, refund on a failed create, joins and the lobby
past the cap, rollover on the next UTC day, `/api/admin/usage` auth), and moderation: a kick
closes the socket and the player cannot rejoin by token, key, name or as a
spectator, also after a restart; listed rooms start with chat off and refuse
chat until the host turns it on; blocked names are refused in listed rooms
only). Nothing is deployed by it.

`scripts/online-e2e.sh` plays the desktop app against that Worker: it starts
`wrangler dev --local` with the production settings (uploads off, Turnstile
required) and a loopback Turnstile stand-in, then runs the ignored test
`e2e::online::` (see [TESTING.md](TESTING.md)). About a minute; nothing is
deployed.

## Limits

- One process, one machine, local disk. A second server is a separate
  service with its own rooms. Moving to a database and shared object storage is
  future work (see [the architecture](ARCHITECTURE.md)).
- No accounts or per-user quotas. Moderation is the host's: **Remove…** in
  the player list bans a name and its device key from that room, and the host
  can switch room chat off. This server has no public lobby.
- Rate limits are in memory and per address, so they reset on restart and do
  not tell apart people sharing one address.
