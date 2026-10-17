#!/usr/bin/env python3
"""Regenerate packaging/ffmpeg-pins.txt from a BtbN/FFmpeg-Builds release.

    scripts/update-ffmpeg-pins.py autobuild-2026-10-08-13-05 [--branch 8.1]

Reads the release's own checksums.sha256, so nothing large is downloaded.
Prefers the release-branch build (n<branch>.*-lgpl-<branch>) over master.
"""
import argparse
import re
import sys
import urllib.request

REPO = "https://github.com/BtbN/FFmpeg-Builds/releases/download"
TARGETS = ["linux64", "linuxarm64", "win64", "winarm64"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("tag", help="release tag, e.g. autobuild-2026-10-08-13-05")
    parser.add_argument("--branch", default="8.1", help="FFmpeg release branch (default 8.1)")
    parser.add_argument("--output", default="packaging/ffmpeg-pins.txt")
    args = parser.parse_args()

    with urllib.request.urlopen(f"{REPO}/{args.tag}/checksums.sha256", timeout=60) as response:
        checksums = response.read().decode()
    pins, version = [], None
    for target in TARGETS:
        pattern = re.compile(
            rf"^([0-9a-f]{{64}})\s+(ffmpeg-n{re.escape(args.branch)}\.[^\s]*-{target}-lgpl-{re.escape(args.branch)}\.(?:tar\.xz|zip))$",
            re.M,
        )
        match = pattern.search(checksums)
        if not match:
            sys.exit(f"no {target} LGPL build for FFmpeg {args.branch} in {args.tag}")
        digest, name = match.groups()
        version = re.match(r"ffmpeg-(n[^-]+(?:-\d+-g[0-9a-f]+)?)-", name).group(1)
        pins.append(f"{target} {digest} {REPO}/{args.tag}/{name}")
    header = f"""# Pinned FFmpeg builds for the optional downstream bundle path.
# Official PieceOff release bundles exclude FFmpeg by default. Edit with
# scripts/update-ffmpeg-pins.py, not by hand: each line is
#   <target> <sha256> <url>
# FFmpeg {version}, LGPLv3 static builds from
# https://github.com/BtbN/FFmpeg-Builds, release {args.tag}.
# The archive's SHA-256 is checked before anything is unpacked.
# macOS has no pinned build: see docs/FFMPEG.md.
"""
    with open(args.output, "w") as out:
        out.write(header + "\n".join(pins) + "\n")
    print(f"wrote {args.output} for {version} ({args.tag})")


if __name__ == "__main__":
    main()
