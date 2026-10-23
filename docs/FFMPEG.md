# FFmpeg

PieceOff decodes video, audio, and animated APNG/GIF by running `ffmpeg` and
`ffprobe` as separate programs. It does not link FFmpeg. Still images and
animated WebP use native Rust decoders and do not need FFmpeg.

## Release policy

Official release bundles include a **patent-light** FFmpeg: `ffmpeg` and
`ffprobe` in `ffmpeg/`, and the corresponding source in `source/`. It is built
by `scripts/build-ffmpeg.sh` from the unmodified upstream release pinned in
[`packaging/ffmpeg-source.txt`](../packaging/ffmpeg-source.txt):

- **License: LGPL 2.1 or later.** `--disable-gpl --disable-nonfree
  --disable-autodetect`: no GPL or nonfree code, no system libraries, hardware
  APIs, or external codec libraries. The script fails the build if the result
  reports a GPL or nonfree option.
- **No patent-encumbered codecs.** The build disables the decoders for H.264,
  HEVC, VVC, VC-1, AAC and E-AC-3, and the AAC encoder. These formats are
  covered by active patent pools that PieceOff does not license, and the
  project (including its paid hosted rooms) does not want to be their
  distributor. The script checks that those decoders are really absent.
  Demuxers and parsers stay, so `ffprobe` can still name a file's codec.
- **One statically linked dependency: zlib** (zlib license), built from the
  pinned release, for PNG/APNG and compressed MP4 headers.
- **Corresponding source:** `source/` holds the pristine FFmpeg and zlib
  tarballs, their signatures, a `SHA256SUMS.txt`, the exact
  `build-ffmpeg.sh`, and the pin file. No patches are applied.
  `ffmpeg/FFMPEG-BUILD.txt` records the configure options.
- **Replaceable:** PieceOff runs FFmpeg as a separate program, so anyone can
  drop in their own `ffmpeg` and `ffprobe`, or choose a folder under **You →
  Media tools → Choose folder…**.

What the bundled build plays: VP8, VP9, MPEG-1/2/4, ProRes, GIF, WebP,
APNG/PNG, MP3, Opus, Vorbis, FLAC, AC-3 and the common containers. What it
does not: H.264/HEVC video (most MP4, MOV and MKV files) and AAC audio. Clip
cutting uses the native MPEG-4 video encoder and, without AAC, FFmpeg's native
Opus encoder (`-strict -2`); with a full FFmpeg it uses AAC again.

## The in-app codec install

When a player opens a video the current FFmpeg cannot decode (the app compares
ffprobe's codec names with `ffmpeg -decoders`), it shows a consent window and,
on **Install video codecs**, downloads a full FFmpeg from a third party
straight to the player's machine (`crates/pieceoff-app/src/codec_pack.rs`,
`codec_ui.rs`). PieceOff does not host or redistribute those binaries:

- Each file is a **pinned URL with a pinned SHA-256**, on a GitHub release with
  a fixed tag (so it does not rot): Windows, the gyan.dev essentials build via
  `GyanD/codexffmpeg`; Linux and macOS, `eugeneware/ffmpeg-static` b6.1.1. A
  mismatch deletes the download and installs nothing.
- Only the two named programs are extracted, with a size cap, into the
  player's data folder (`<data>/codecs`), then run once to check they work and
  have the codecs. The previous install is replaced only on success.
- The consent text names the host, the size, the license (GPL v3) and says
  that patent licensing where the player lives is their responsibility.
- **Media tools** shows the state, offers the install, and removes it.
- Search order: chosen folder, environment, **downloaded codecs**, bundled,
  `PATH`.

To update a pin, choose a new tag, copy the SHA-256 GitHub shows for each
asset (or compute it), edit `Pack::for_target`, and run
`cargo test -p pieceoff-app -- --ignored real_pack` on each platform you can.
The manifest unit test checks that every pin is https, hex, and sized.

**Hosted rooms never decode media.** The room server and the Worker relay and
store bytes only; FFmpeg is used by the desktop app on the player's own
machine. Adding server-side transcoding or thumbnailing would make the hosted
service a codec distributor, so do not without a license review.

The app can discover tools in this order, separately for each executable:

1. The folder selected in the app, saved in `settings.json` as `ffmpeg_dir`.
2. `PIECEOFF_FFMPEG` / `PIECEOFF_FFPROBE` full paths, then
   `PIECEOFF_FFMPEG_DIR` for a folder containing both.
3. Beside the app, in its `ffmpeg/` or `bin/` subfolder, or in a macOS
   application bundle's `Contents/Resources` or `Contents/Resources/ffmpeg`.
4. `PATH`.

## Building

```sh
scripts/build-ffmpeg.sh <linux64|linuxarm64|macos|win64> <dest>
```

Needs a C toolchain, `make`, `curl`, `xz`, and `nasm` on x86 (macOS Apple
silicon and Linux arm64 do not need it). `win64` builds under MSYS2 MINGW64
(as the release workflow does), or cross-builds on Linux with mingw-w64.
`scripts/bundle.sh`, `scripts/bundle.ps1 -FfmpegStage`, and
`scripts/bundle-windows-cross.sh` call it. `--no-ffmpeg` (`-NoFfmpeg`) leaves
FFmpeg out, for example in the CI test bundles. `PIECEOFF_FFMPEG_NO_ASM=1`
skips the x86 assembly and the `nasm` requirement for a slower local build;
do not use it for a release.

## Updating the pins

Download the new release and its `.asc` from <https://ffmpeg.org/releases/>
(zlib: <https://zlib.net/>), verify the signature with `gpg` against the keys
named in `packaging/ffmpeg-source.txt`, then change the version, SHA-256, and
URL lines. Run the app's e2e suite against the new build
(`PATH=<dest>/ffmpeg:$PATH cargo test -p pieceoff-app`) before releasing.

## Codec patent review

FFmpeg is licensed under the LGPL, but copyright licensing is separate from
patents. PieceOff's own bundles ship none of the pooled codecs (above), and the
in-app install puts the choice and the download with the player. This reduces,
and does not remove, exposure: this project has not determined patent coverage
for any codec, jurisdiction, or business model, and nothing here is legal
advice. For example, Access Advance describes its HEVC Advance pool as covering
essential HEVC patents and says product implementations with HEVC encoding or
decoding may need a license ([program overview](https://accessadvance.com/licensing-programs/hevc-advance/)),
though it states it will not seek royalties on software downloaded after a
device's sale that decodes purely in software; Via LA's AVC and AAC programs
license makers of end-user decoder products. A decision to ship any of the
disabled decoders again, or to transcode on the hosted service, needs review by
counsel in the target markets first. To disable more, extend
`PATENTED_DECODERS` in `scripts/build-ffmpeg.sh`.
