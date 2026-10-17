# Built-in puzzle media

PieceOff bundles eight original, silent procedural animated-WebP puzzles:
**Slow Orbit**, **Tidal Glass**, **Aurora Veil**, **Dune Light**, **Leaf
Garden**, **Glass Garden**, **Deep Sea Bloom**, and **Geometric Bloom**. The
scenes are abstract and do not depict real places or use third-party footage,
photographs, music, fonts, or copied artwork. Each is a 480×320, six-second
loop with a cyclic animation phase; the last frame is checked against the
normal frame-to-frame motion at the loop boundary. The app's native animated-
WebP decoder plays them without FFmpeg.

The checked-in generated assets and their recipe are offered under the
repository's Apache-2.0 license, consistent with `LICENSE`. To regenerate
them, use Python 3.9–3.12 and FFmpeg built with the `libwebp_anim` encoder.
Install the pinned maintainer dependency and run the generator:

```sh
python3 -m venv .venv-media
. .venv-media/bin/activate
python -m pip install -r scripts/requirements-media.txt
scripts/generate-builtin-media.sh
```

The generator uses NumPy for procedural rasterization. NumPy is BSD-3-Clause
licensed and maintainer-only; ordinary app builds use the checked-in WebP
files and need neither NumPy nor FFmpeg.

All eight assets are bundled in the app binary and copied into the user's
library data directory when selected. To preserve the base game's
self-contained media set, future additions should be original procedural work
or have reviewed redistribution terms recorded here.
