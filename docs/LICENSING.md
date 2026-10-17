# Licensing and third-party notices

PieceOff's original source code and documentation are licensed under the
Apache License, Version 2.0. See the repository's `LICENSE` file. The license
includes an express patent license and a termination provision for patent
litigation. The project does not require copyright assignment; contributors
retain copyright in their contributions.

## Open-source and hosted-service model

The desktop app and room server are open source under Apache-2.0. The license
allows anyone to use, modify, redistribute, and run them, including for
commercial purposes, provided its terms and the applicable third-party
licenses are followed. PieceOff does not charge a license fee or royalty for
self-hosting; other parties may also offer hosting or services based on the
software under the license terms.

The project may separately operate an optional hosted service at
[pieceoff.app](https://pieceoff.app). A hosted service can charge for hosting,
storage, or other services under separate service terms; that does not change
the Apache-2.0 rights to the software or prevent independent self-hosting.
The domain is the intended home for the service; this repository does not
promise that hosted rooms, any particular features, or pricing are currently
available. Service terms and privacy information should be published before
accepting hosted users or payments.

Contributions are submitted under Apache-2.0 as described in
[`CONTRIBUTING.md`](../CONTRIBUTING.md). Contributors keep their copyright.
The project does not currently require a separate commercial license or
copyright assignment, and does not promise contributor royalties, revenue
sharing, or employment. Because the project does not collect a separate
commercial license grant, do not assume it can offer all contributions under
different terms.

Third-party components are not relicensed under PieceOff's license. Their
licenses and required notices continue to apply. The release bundles include
a generated Rust dependency inventory (`THIRD_PARTY_LICENSES.txt`) and
third-party license texts/notices (`THIRD_PARTY_NOTICES.txt`). Official
bundles exclude FFmpeg; it is an external executable selected by the user.
An explicit downstream bundle option exists, but it remains uncleared until
the complete corresponding source/build materials are reviewed. See [FFmpeg
distribution notes](FFMPEG.md) before bundling or redistributing it.
These generated files help preserve notices but do not replace reviewing the
exact components and obligations in each release.

The application includes egui's default fonts, which have separate OFL-1.1
and Ubuntu Font License terms. It also includes eight original procedural
animated WebP puzzles under Apache-2.0; see [built-in puzzle media](BUILTIN_MEDIA.md)
for their recipe and provenance. The maintainer-only generator uses NumPy
(BSD-3-Clause), which is not shipped with the app. Check media, fonts, codecs,
operating-system libraries, and additional assets separately before
redistribution.

The project name and logo, if added, are not granted as a trademark by the
Apache-2.0 license. Do not imply endorsement or affiliation when redistributing
modified builds.
