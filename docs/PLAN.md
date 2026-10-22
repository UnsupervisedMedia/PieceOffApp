# Build plan

## Product direction

Video and cooperative play are the headline features: import a clip, choose a loop segment, cut it into pieces, invite friends, and solve while all pieces display the same moment in the clip. Controls: play/pause, scrub, loop in/out, speed, mute/volume, and a still-frame assist. Source media must survive moving the puzzle between computers by using a self-contained package or an explicit relink flow.

Classic image puzzles remain a first-class mode. Gameplay should keep familiar drag, snap, pan, zoom, preview, collection, and save/resume behavior while improving responsiveness, discoverability, and large-puzzle handling.

Initial clients target Linux, Windows, and macOS desktop. The multiplayer protocol must be independent of the desktop client so web and mobile clients can follow. The server is part of the product from the first playable release.

PieceOff's app and room server remain open source under Apache-2.0. Players and
communities may run their own servers, including commercially. The project may
also offer optional managed rooms at `pieceoff.app`; hosted plans can charge
for service and convenience, but do not restrict the right to self-host. See
the [licensing and hosted-service model](LICENSING.md).

## Milestones and gates

1. **Shared static puzzle foundation — prototype complete.** Versioned protocol, room server, image import, deterministic jigsaw contours, linked groups, shared loose-piece tray, drag leases, snapping, Ctrl-click multi-selection, and JSON room save/reload. Gate verified by core shape-compatibility tests and two-client WebSocket tests. Board pan/zoom is implemented in the desktop client. Fuller network recovery remains.
2. **Shared video technical spike — prototype complete.** FFmpeg decodes one frame stream and FFmpeg decodes optional audio that each client plays through its own audio device; all pieces sample from one shared texture; play/pause, seek, loop in/out, and speed propagate through the room timeline. Local volume/mute, still-frame assist, and periodic threshold-based A/V drift correction are available. Remaining: full two-client A/V playback exercise, stronger decoder recovery, and FFmpeg packaging.
3. **First product slice: multiplayer looping video — implementation in progress.** Create room, invite by code, loop controls, scrub, speed, audio, still-frame assist, completed-image reference view, automatic reconnect with bounded backoff, server-managed, resumable media transfer of multi-gigabyte files, and self-contained `.pieceoff` puzzle packages are implemented. Packages preserve board progress and media while assigning a new room identity on rehost. Automated WebSocket coverage checks that two clients receive the same host-controlled play/pause, seek, rate, and loop state, and that the saved state survives reload. The server enforces a configurable aggregate media quota and expires inactive rooms with unreferenced media cleanup. An automated end-to-end suite (real server, real FFmpeg, two clients; `docs/TESTING.md`) now exercises create, solve, leave, rejoin, server restart and crash, reconnect, host handoff during upload, and decoder recovery, and found and fixed an event-ordering race; decoder recovery gained a hung-process watchdog, an empty-video guard, and automatic restarts. Gate: two users create, solve, leave, and rejoin a video puzzle with progress and loop settings intact. This passes in the automated setup; what remains is a session with two people, speakers, and two machines to judge audio sync and feel.
4. **Feature parity.** The video queue with hot swap, repeat modes, and a video lock is implemented. Ctrl-click multi-selection with shared group movement is implemented. Localization and accessibility improvements, additional slicers, and further parity work remain; use `FEATURE_MATRIX.md` to track implementation or explicit deferrals.
5. **Release readiness.** Native packages for three desktop OSs; server deployment, codec/runtime and license review; crash recovery; performance budget on large puzzles; migration/import where feasible. Gate: clean install and end-to-end room play on each OS.

## Hosted rooms on Cloudflare (in progress)

The optional managed service at `pieceoff.app` runs the same room rules as the
self-hosted server: one Rust room core, hosted by axum or by a Cloudflare
Durable Object, with media in R2. Status as of 2026-10-22:

- **Done.** Room rules (`RoomRuntime::apply`) and a wasm room crate in core
  (#18, merged). Join/leave/host handoff in core, plus a TypeScript Durable
  Object host with hibernating sockets, SQLite write-behind, reconnect with a
  secret resume token, and liveness sweeps. Resumable multipart media in R2
  with quota and ranged download, expiry of empty and unclaimed rooms with R2
  cleanup, and Turnstile, rate limits, origin checks and socket caps. The
  worker has unit and integration tests against a local `wrangler dev`
  (`scripts/worker-test.sh`). Setup and endpoints: [DEPLOYMENT.md](DEPLOYMENT.md).
- **On hold: go-live.** Needs the owner: create the R2 bucket `pieceoff-media`,
  the Turnstile widget and secret, and wire the rooms Worker into the connected
  GitHub to Cloudflare Workers Builds path (no direct `wrangler deploy`).
  Nothing has been deployed.
- **Next: Plan 1C.** Protocol v9 (`seed_sha256`, `still_sha256`, owner and
  source fields); completion still and release of the finished video so a solved
  room keeps only a still; restart of a released item accepted only if the
  re-upload matches `seed_sha256`; quota-freeing media deletion. Move ephemeral
  messages (cursors, chat, pings) behind a shared core `relay()` so the Durable
  Object host relays them like the axum server does; the wasm `Room` currently
  answers those with an error.
- **Later: Plan 1D, web client.** egui compiled to WASM sharing the desktop board
  code, a browser `MediaDecoderBackend` (hidden `<video>` uploaded to GL through
  an `egui_glow` paint callback; the spike measured 60 fps at 400 pieces),
  `/r/<CODE>` share links with an "Open in PieceOff" hand-off, the Turnstile
  widget, and a measured cold-load size. Desktop browsers only for v1; a
  phone/tablet client and joining self-hosted servers from the web client are
  out of scope for now.
- **Known gaps to schedule.** The upload token is not rotated when a host
  leaves with nobody to take over; unknown-room lookups reach the Durable
  Object before the lookup limiter; Turnstile `hostname` and `action` are not
  checked; one wasm instance is shared by all rooms in an isolate; native
  Windows and macOS launches are still unverified. The design spec and plans
  for this work live in `docs/superpowers/`, which is gitignored, so they exist
  only on the author's machine until that is decided.

## Release readiness

- Complete a manual two-person video/audio playtest on separate machines, including seek and loop-wrap synchronization (`docs/TESTING.md`).
- Verify native Windows and macOS builds and launches. Linux tests and release builds pass; the Windows GNU build is a cross-build only, and macOS requires an Apple toolchain.
- Continue classifying each `FEATURE_MATRIX.md` item as implemented, deferred, or out of scope.
- For each new dependency, asset, or native component, review its license and packaging requirements. Record package schema changes and migration behavior.

Development should continue in small, reviewable vertical slices. Keep media decoding behind `MediaDecoderBackend` so core rules do not depend on a specific codec stack.
