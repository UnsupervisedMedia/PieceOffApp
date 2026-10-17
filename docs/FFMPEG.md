# FFmpeg

PieceOff decodes video, audio, and animated APNG/GIF by running `ffmpeg` and
`ffprobe` as separate programs. It does not link FFmpeg. Still images and
animated WebP use native Rust decoders and do not need FFmpeg.

## Release policy

Official release bundles exclude FFmpeg. Users install their own FFmpeg build
and select its folder under **You → Media tools → Choose folder…** (or put
`ffmpeg` and `ffprobe` on `PATH`). PieceOff does not distribute codec
implementations in its standard bundles.

The app can discover tools in this order, separately for each executable:

1. The folder selected in the app, saved in `settings.json` as `ffmpeg_dir`.
2. `PIECEOFF_FFMPEG` / `PIECEOFF_FFPROBE` full paths, then
   `PIECEOFF_FFMPEG_DIR` for a folder containing both.
3. Beside the app, in its `ffmpeg/` or `bin/` subfolder, or in a macOS
   application bundle's `Contents/Resources` or `Contents/Resources/ffmpeg`.
4. `PATH`.

## Optional downstream FFmpeg bundle

The Linux and Windows scripts retain an explicit `--with-ffmpeg` /
`-WithFfmpeg` option for downstream maintainers who independently review the
exact build. The scripts check that
`PIECEOFF_FFMPEG_SOURCE_ARCHIVE` points to a `.tar.xz`, `.tar.gz`, or `.zip`,
then copy it beside the executable with a checksum. This is only a mechanical
presence check; it does not verify that the archive is complete or compliant.
The archive must include the exact FFmpeg source, build scripts and
configuration, patches, source and notices for statically included
dependencies, and all other corresponding materials required by the
applicable licenses. Do not opt in to bundling until that archive is validated
for the exact binary.

FFmpeg's license depends on the configure flags and optional libraries. Its
[official licensing guidance](https://ffmpeg.org/legal.html) says GPL-only
components make the FFmpeg build GPL, and advises checking `--enable-gpl` and
`--enable-nonfree`. An LGPL label or an upstream archive name is not by itself
proof that all binary obligations have been met. The project-pinned builds in
[`packaging/ffmpeg-pins.txt`](../packaging/ffmpeg-pins.txt) are static BtbN
builds. Their source/build archive has not been staged or reviewed, so the
opt-in path is not cleared for official PieceOff releases.

For a custom macOS build, set `FFMPEG_DIR` to a folder containing `ffmpeg`,
`ffprobe`, its license file (`LICENSE`, `LICENSE.txt`, or
`LICENSE-FFmpeg.txt`), and `FFMPEG-BUILD.txt`. The same corresponding-source
archive gate applies. An FFmpeg executable inside a macOS app bundle also
needs to meet Apple's signing and notarization requirements for that package.

## Codec patent review

PieceOff does not ship codecs in official bundles, but FFmpeg builds users
choose may include codecs with separate patent considerations. This project
has not determined patent coverage for any codec, jurisdiction, product, or
business model. For example, Access Advance describes its HEVC Advance pool
as covering essential HEVC patents and says product implementations with
HEVC encoding or decoding may need a license ([program overview](https://accessadvance.com/licensing-programs/hevc-advance/)); the Alliance for Open
Media describes AV1 as available under its Open Media Patent License 1.0
([AOMedia overview](https://aomedia.org/about/story/)). These are program
descriptions, not a legal determination for PieceOff. A commercial or
preinstalled-codec distribution needs review by counsel in its target markets.

## Updating pins

Pins are used only for the optional downstream path. To update them, use the
upstream release checksums and rerun the bundle review:

```sh
scripts/update-ffmpeg-pins.py autobuild-YYYY-MM-DD-HH-MM --branch 8.1
```

The optional binary/source pair must be tested together. Linux and Windows
GNU test bundles currently exclude FFmpeg; native Windows and macOS bundles
have not been launched on those systems.
