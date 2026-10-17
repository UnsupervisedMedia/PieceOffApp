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

## Release readiness

- Complete a manual two-person video/audio playtest on separate machines, including seek and loop-wrap synchronization (`docs/TESTING.md`).
- Verify native Windows and macOS builds and launches. Linux tests and release builds pass; the Windows GNU build is a cross-build only, and macOS requires an Apple toolchain.
- Continue classifying each `FEATURE_MATRIX.md` item as implemented, deferred, or out of scope.
- For each new dependency, asset, or native component, review its license and packaging requirements. Record package schema changes and migration behavior.

Development should continue in small, reviewable vertical slices. Keep media decoding behind `MediaDecoderBackend` so core rules do not depend on a specific codec stack.
