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
moderation: see [Security notes](#security-notes).

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

## Cloudflare (planned)

A hosted instance is planned on Cloudflare Workers, Durable Objects (one per
room) and R2; the axum server above is unchanged. The room rules run in the
Durable Object as the `pieceoff-room-wasm` module.

Worker size gate, measured with `scripts/wasm-size.sh` on 2026-10-21 (no
`wasm-opt`, no brotli installed): **288,099 bytes raw, 96,851 bytes gzip**.
The free-plan Worker limit is 3 MB compressed, so the room core uses about 3%
of it.

Local development: `scripts/worker-test.sh` builds the wasm module
(`worker/build.sh`), type-checks `worker/`, and runs a two-client test against
`wrangler dev --local` (join, piece leases, release on disconnect, bad frames,
unknown codes, restart persistence). Nothing is deployed by it. Rooms are
created with `POST /api/rooms` (a `create_room` body; returns `code` and the
creator's `client_id`), then clients open `/ws?code=<CODE>[&client=<id>]` and
send `join_room` first. Not yet in the worker: R2 media transfer, Turnstile and
per-IP limits, quota and idle cleanup. Production publishing of anything under
`pieceoff.app` goes through the connected GitHub to Cloudflare Workers Builds
path, not a direct `wrangler deploy`.

Plan limits checked against Cloudflare's pricing pages on 2026-10-21: Durable
Object requests count HTTP requests, RPC calls, WebSocket messages (20 : 1)
and alarm invocations; each `setAlarm()` is a row written; the free plan allows
100,000 requests, 13,000 GB-s, and 100,000 SQLite rows written per day, and
only SQLite-backed Durable Objects. R2 is free to 10 GB with free egress.

## Limits

- One process, one machine, local disk. A second server is a separate
  service with its own rooms. Moving to a database and shared object storage is
  future work (see [the architecture](ARCHITECTURE.md)).
- No accounts, moderation, or per-user quotas.
- Rate limits are in memory and per address, so they reset on restart and do
  not tell apart people sharing one address.
