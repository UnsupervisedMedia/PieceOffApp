# Contributor and agent guidance

Before implementing a feature, read `README.md`, `docs/PLAN.md`,
`docs/FEATURE_MATRIX.md`, `docs/ARCHITECTURE.md`, and `docs/LICENSING.md`.

PieceOff is an independent Rust implementation. Work from this repository's
requirements; do not copy third-party code, assets, UI text, or documentation
without confirming the applicable rights and license. If legacy file
compatibility is needed, describe observable behavior and write an original
parser.

## Product and architecture

- Prioritize smooth looping video puzzles, reliable drag and snap, and shared
  multiplayer rooms. Keep still images supported by the same puzzle model.
- The server owns room state and validates moves. Clients may render
  optimistically, then reconcile with server updates.
- Keep media decoding behind `MediaDecoderBackend`; core puzzle rules must not
  depend on a codec implementation.
- Make changes in small vertical slices that are playable or inspectable.
- Pin dependency versions when adding dependencies, and review their licenses,
  native build requirements, and packaging needs.

The current prototype uses an eframe/egui desktop client, an axum WebSocket
room server (protocol v12), FFmpeg for video and audio decoding, cpal for audio
output, native animated WebP decoding, deterministic jigsaw contours, linked
piece groups, per-player piece holders, resumable invite-gated media transfer,
`.pieceoff` packages, a local puzzle library, rebindable controls, and durable
room snapshots. Release
bundles exclude FFmpeg by default; see `docs/FFMPEG.md` before changing that.

## Build and verification

Run `scripts/check.sh` before submitting a code change. It runs formatting,
clippy, tests, and a Windows GNU cross-check. `scripts/check.sh --bundle` also
rebuilds the Windows GNU bundle. A cross-built Windows executable has not been
launched on Windows; use native platform builds and launches as separate
verification.

Build on Linux first and use CI for Windows and macOS. For headless GUI
inspection, see `scripts/visual-dev.sh`. Keep the two-client WebSocket and
looping-video integration coverage working.

## Release-readiness work

- Perform the manual two-person video playtest on separate machines, including
  audio synchronization after seeks and loop wraps (`docs/TESTING.md`).
- Verify native Windows and macOS builds and launches.
- Keep `docs/FEATURE_MATRIX.md` current; mark remaining parity work as
  implemented, deferred, or out of scope.
- Keep the license and contribution policy consistent with `LICENSE` and
  `CONTRIBUTING.md`.
