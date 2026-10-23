# Build and packaging guide

PieceOff currently produces two native executables: `pieceoff-app` for each
player and `pieceoff-server` for the room host. Build on each target operating
system; cross-compilation is not yet part of the release process.

## Build

Install Rust 1.87 or newer and build from the repository root:

```sh
cargo build --workspace --release --locked
```

The app and server appear in `target/release/` (with `.exe` suffixes on
Windows). Build separately on Linux, Windows, and macOS. The GitHub Actions
workflow is configured to test and build on all three platforms; native
release builds should be made on their target OS.

### Windows cross-build from Ubuntu

Part of the normal process: `scripts/check.sh` runs fmt, clippy, the tests, and
a `cargo check --all-targets` for `x86_64-pc-windows-gnu`, so a change that only
breaks Windows is caught before a push. `scripts/check.sh --bundle` also
assembles the Windows zip, and `scripts/bundle-windows-cross.sh` does just that:
`dist/PieceOff-<version>-win64-gnu.zip` with both `.exe` files, a cross-built
Windows FFmpeg with its source, licenses, docs, `pieceoff.ps1` and `START-HERE.txt`. Windows
artifacts build in `target/windows` (`PIECEOFF_WINDOWS_TARGET_DIR` overrides).

```sh
sudo apt-get install gcc-mingw-w64-x86-64 zip nasm
rustup target add x86_64-pc-windows-gnu
scripts/check.sh --bundle
```

This proves the tree compiles and links. Nothing is launched, so it does not
replace a native Windows build or runtime test.

## Releases (GitHub Actions)

`.github/workflows/release.yml` builds native bundles for Linux x86_64,
Windows x86_64, macOS Apple silicon, and macOS Intel when a `v*` tag is pushed.
The tag must equal `v` plus the version in `crates/pieceoff-app/Cargo.toml`. The
workflow uploads the archives, a `SHA256SUMS.txt`, a build-provenance
attestation, and a **draft** release for a maintainer to review and publish.
Bundles include a basic FFmpeg and its corresponding source; see [FFMPEG.md](FFMPEG.md). Run the workflow manually
(`workflow_dispatch`) to build bundles without publishing.

## Assemble a release bundle

```sh
scripts/bundle.sh                # Linux/macOS bundle, with FFmpeg built from source
.\scripts\bundle.ps1 -FfmpegStage dist/ffmpeg-stage   # Windows bundle (see below)
```

The script builds release binaries and includes both the dependency license
inventory and full third-party notices. A basic FFmpeg (without H.264/HEVC/AAC) is included by default:
`scripts/build-ffmpeg.sh` builds it from the pinned upstream release
(`packaging/ffmpeg-source.txt`) and stages `ffmpeg/` and `source/` (the
pristine tarballs, signatures, checksums, and the build script), which the
bundle scripts copy in. A first build takes a few minutes and needs a C
toolchain, `make`, and `nasm` on x86. Pass `--no-ffmpeg` (PowerShell:
`-NoFfmpeg`) to leave it out. On Windows, build FFmpeg first in an MSYS2
MINGW64 shell (`scripts/build-ffmpeg.sh win64 dist/ffmpeg-stage`) and pass that
folder as `-FfmpegStage`; the release workflow does the same. See
[FFmpeg](FFMPEG.md) for the license and update procedure.

The bundle includes `pieceoff.sh` (or `pieceoff.ps1` on Windows), which starts
and stops the app and local room server together. The room host runs the
server; every participant runs the app. Release bundles carry their own basic
FFmpeg (the app offers a full one on demand); source builds need `ffmpeg` and `ffprobe` on `PATH` for video and
animated APNG/GIF puzzles; source audio plays through the system audio device.
Animated WebP uses the native Rust image decoder. Video files can use
MP4, M4V, MOV, MKV, WebM, AVI, MPEG, OGV, FLV, 3GP, WMV, TS, MTS, or M2TS
containers when the installed FFmpeg build includes their codecs. Building on Linux needs the ALSA development headers. Source builds do not bundle
FFmpeg or codecs; release bundles include the FFmpeg described in [FFMPEG.md](FFMPEG.md). Video frames are decoded at source size up to 1920×1080 and
downscaled above that ceiling to keep texture and frame-buffer memory bounded;
the original file is preserved for upload and puzzle packages.
Imported media is limited to 8192 pixels per side and 40 megapixels total.
Images are checked against these limits before frame decoding.

The host uploads its selected media to the server (up to the server's `PIECEOFF_MAX_MEDIA_BYTES`, 16 GiB by default).
Guests can join without choosing a local copy; downloads are cached under the
system temporary directory, or under `PIECEOFF_CACHE_DIR` if set. A matching
local file skips the download. The server stores room snapshots in
`pieceoff-data/` and media under `pieceoff-data/media/` by default. Set
`PIECEOFF_DATA_DIR` to choose a persistent location and `PIECEOFF_BIND` to
configure its listen address.

In the app, **Save package…** writes a portable `.pieceoff` file containing the
current puzzle progress and embedded media. Move that file to another PieceOff
client, choose **Open package…**, then create a room to rehost the puzzle on the
selected server. Package reads verify the embedded asset's SHA-256 and write it
to the same content-addressed cache used for room downloads.

## OS release checks

- Linux: build and launch on the oldest distribution you intend to support;
  document any required system GUI libraries alongside the bundle.
- Windows: build and launch on a clean supported Windows install; verify
  `ffmpeg.exe` and `ffprobe.exe` are discoverable through `PATH`
  for video and audio.
- macOS: build and launch on each supported architecture; sign and notarize
  distributed app bundles before public release.

The CI matrix is configured to verify compilation and tests, not GUI launch,
signing, notarization, FFmpeg availability, or full video playback on each OS.
Those remain release acceptance checks. Successful runs attach 14-day test
bundles containing the two executables and quick-start/build notes; these are
for manual testing, not production distribution.

## Room hosting

The room server is deployed separately from the desktop bundle. See
[Deploying the room server](DEPLOYMENT.md) for the Docker image with automatic
HTTPS, the systemd unit, reverse-proxy requirements, every `PIECEOFF_*`
setting, backups, and security notes. For a quick trusted-network test the
bundle's `pieceoff.sh` starts a local server on port `28878`; it has no TLS and
no account system, so do not expose that directly to the internet.
