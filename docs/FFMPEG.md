# FFmpeg

PieceOff decodes video, audio, and animated APNG/GIF by running `ffmpeg` and
`ffprobe` as separate programs. It does not link FFmpeg. Still images and
animated WebP use native Rust decoders and do not need FFmpeg.

## Release policy

Official release bundles include FFmpeg: `ffmpeg` and `ffprobe` in `ffmpeg/`,
and the corresponding source in `source/`. The programs are built by
`scripts/build-ffmpeg.sh` from the unmodified upstream release pinned in
[`packaging/ffmpeg-source.txt`](../packaging/ffmpeg-source.txt):

- **License: LGPL 2.1 or later.** The build uses `--disable-gpl
  --disable-nonfree --disable-autodetect`, so it contains no GPL or nonfree
  code and links no system libraries, hardware APIs, or external codec
  libraries. The script fails the build if the result reports a GPL or nonfree
  option.
- **One statically linked dependency: zlib** (zlib license), built by the same
  script from the pinned release. FFmpeg needs it for PNG/APNG and compressed
  MP4 headers. Its license text ships beside the programs.
- **Corresponding source:** `source/` holds the pristine FFmpeg and zlib
  tarballs, their detached signatures, a `SHA256SUMS.txt`, the exact
  `build-ffmpeg.sh` that produced the programs, and the pin file. No patches
  are applied. `ffmpeg/FFMPEG-BUILD.txt` records the configure options.
- **Replaceable:** PieceOff runs FFmpeg as a separate program, so anyone can
  drop in their own `ffmpeg` and `ffprobe`, or choose a folder under **You →
  Media tools → Choose folder…**.

Source builds of PieceOff do not include FFmpeg. Install it and put `ffmpeg`
and `ffprobe` on `PATH`, or choose its folder in the app.

The app can discover tools in this order, separately for each executable:

1. The folder selected in the app, saved in `settings.json` as `ffmpeg_dir`.
2. `PIECEOFF_FFMPEG` / `PIECEOFF_FFPROBE` full paths, then
   `PIECEOFF_FFMPEG_DIR` for a folder containing both.
3. Beside the app, in its `ffmpeg/` or `bin/` subfolder, or in a macOS
   application bundle's `Contents/Resources` or `Contents/Resources/ffmpeg`.
4. `PATH`.

## What the build can decode and encode

FFmpeg's native decoders cover H.264, HEVC, VP8, VP9, ProRes, MPEG-1/2/4,
GIF, WebP, APNG/PNG, AAC, MP3, Opus, Vorbis, FLAC, and the common containers.
Not included, because they need external libraries: software AV1 decoding
(FFmpeg's native AV1 decoder needs hardware), and the x264/x265/OpenH264/libvpx
encoders. Clip cutting uses the native MPEG-4 and AAC encoders, which every
FFmpeg build has. Adding dav1d (BSD-2-Clause) for AV1 is the obvious next
step: pin its source in `packaging/ffmpeg-source.txt`, build it in
`scripts/build-ffmpeg.sh` like zlib, and ship its source and license.

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
patents. The bundled build includes decoders for formats such as H.264, HEVC,
and AAC, so PieceOff releases do distribute codec implementations. This
project has not determined patent coverage for any codec, jurisdiction,
product, or business model. For example, Access Advance describes its HEVC
Advance pool as covering essential HEVC patents and says product
implementations with HEVC encoding or decoding may need a license ([program
overview](https://accessadvance.com/licensing-programs/hevc-advance/)); the
Alliance for Open Media describes AV1 as available under its Open Media Patent
License 1.0 ([AOMedia overview](https://aomedia.org/about/story/)). These are
program descriptions, not a legal determination for PieceOff. A commercial or
preinstalled-codec distribution, or a hosted service that transcodes, needs
review by counsel in its target markets. If that review says a codec must go,
add `--disable-decoder=<name>` to the configure options in
`scripts/build-ffmpeg.sh`.
