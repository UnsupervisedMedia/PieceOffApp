#!/usr/bin/env python3
"""Encode the bundled animated-WebP loops as web/public/builtin/<sha256>.mp4 + .jpg.

The sha256 is taken from the .webp bytes (the same key the core's
BUILTIN_MEDIA_SHA256 uses). Frames are decoded with Pillow (ffmpeg cannot decode
animated WebP here) and timed with the file's own per-frame durations, so the
mp4 plays exactly the webp's frames once; the player loops it with `loop`.
"""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
SRC = ROOT / "crates/pieceoff-app/assets/builtin"
OUT = ROOT / "web/public/builtin"
FFMPEG = os.environ.get("FFMPEG", "ffmpeg")


def decode(path):
    """Return ([RGB frame], [duration_ms]) using the webp's real timing."""
    im = Image.open(path)
    frames, durations = [], []
    for i in range(im.n_frames):
        im.seek(i)
        im.load()  # populates info["duration"]
        frames.append(im.convert("RGB"))
        durations.append(int(im.info["duration"]))
    return frames, durations


def encode(sha, frames, durations, tmp):
    w, h = frames[0].size
    if w % 2 or h % 2 or w * 2 != h * 3:
        raise SystemExit(f"{sha}: unexpected frame size {w}x{h} (want even, 3:2)")
    lines = []
    for i, (frame, ms) in enumerate(zip(frames, durations)):
        name = tmp / f"{i:04d}.png"
        frame.save(name)
        lines.append(f"file '{name}'\nduration {ms / 1000:.3f}\n")
    # The concat demuxer drops the last duration unless the file is repeated; the
    # repeat is cut off again by -t (the summed durations) so the loop is exactly N frames.
    lines.append(f"file '{tmp / f'{len(frames) - 1:04d}.png'}'\n")
    listing = tmp / "list.txt"
    listing.write_text("".join(lines))
    mp4 = OUT / f"{sha}.mp4"
    subprocess.run(
        [FFMPEG, "-hide_banner", "-loglevel", "error", "-y",
         "-f", "concat", "-safe", "0", "-r", "12", "-i", str(listing),
         "-t", f"{sum(durations) / 1000:.3f}", "-an", "-c:v", "libx264", "-profile:v", "main", "-pix_fmt", "yuv420p",
         "-preset", "slow", "-crf", "26", "-fps_mode", "passthrough",
         "-video_track_timescale", "1000", "-movflags", "+faststart",
         "-map_metadata", "-1", "-fflags", "+bitexact", "-flags:v", "+bitexact",
         "-threads", "1", str(mp4)],
        check=True,
    )
    frames[0].save(OUT / f"{sha}.jpg", "JPEG", quality=85, optimize=True)
    return mp4


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for webp in sorted(SRC.glob("*.webp")):
        sha = hashlib.sha256(webp.read_bytes()).hexdigest()
        frames, durations = decode(webp)
        with tempfile.TemporaryDirectory() as t:
            mp4 = encode(sha, frames, durations, pathlib.Path(t))
        manifest[sha] = {"mp4": f"{sha}.mp4", "poster": f"{sha}.jpg"}
        print(f"{webp.name}: {sha[:12]} {len(frames)}f {sum(durations)}ms {mp4.stat().st_size // 1024} KiB")
    (OUT / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    # Drop outputs for webps that no longer exist.
    keep = {n for e in manifest.values() for n in e.values()} | {"manifest.json"}
    for stale in OUT.iterdir():
        if stale.name not in keep:
            stale.unlink()
    return 0


if __name__ == "__main__":
    sys.exit(main())
