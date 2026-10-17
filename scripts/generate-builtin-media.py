#!/usr/bin/env python3
"""Render PieceOff's original seamless procedural loops as animated WebP."""
import math
import os
import pathlib
import subprocess

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / "crates/pieceoff-app/assets/builtin"
WIDTH, HEIGHT, FPS, SECONDS = 480, 320, 12, 6
FRAMES = FPS * SECONDS
TAU = 2 * math.pi
Y, X = np.mgrid[0:HEIGHT, 0:WIDTH].astype(np.float32)
U, V = X / WIDTH, Y / HEIGHT


def rgb(r, g, b):
    return np.stack((r, g, b), axis=-1).clip(0, 255).astype(np.uint8)


def blend(a, b, amount):
    t = np.clip(amount, 0, 1)[..., None]
    return a * (1 - t) + b * t


def scene(name, phase):
    if name == "Slow Orbit":
        # Layered interference fields orbit through a soft, luminous palette.
        a = np.sin(TAU * (2.1 * U + .32 * np.sin(TAU * V + phase) + phase / TAU))
        b = np.sin(TAU * (1.6 * V - .28 * np.cos(TAU * U - phase) - phase / TAU))
        c = np.sin(TAU * (1.35 * (U + V) + .16 * np.sin(TAU * (U - V) + phase) - phase / TAU))
        field = (a + b + c) / 3
        glow = np.exp(-(((U - (.5 + .12 * math.cos(phase))) / .28) ** 2
                        + ((V - (.5 + .09 * math.sin(phase))) / .34) ** 2))
        return rgb(122 + 94 * field + 22 * glow,
                   120 + 82 * np.sin(TAU * (field + .24)) + 35 * glow,
                   154 + 90 * np.sin(TAU * (field + .48)) + 22 * glow)

    if name == "Tidal Glass":
        # Long rolling bands, fine caustics, and a slow moving wash of light.
        swell = np.sin(TAU * (3.1 * V + .14 * np.sin(TAU * U + phase)
                               + .05 * np.sin(2 * TAU * U - phase) + .08 * math.sin(phase)))
        swell2 = np.sin(TAU * (5.2 * V + .1 * np.sin(TAU * 1.4 * U - phase)
                               - .06 * math.sin(phase)))
        caustic = (np.maximum(0, np.sin(TAU * (2.3 * U + 1.7 * V) + phase))
                   * np.maximum(0, np.sin(TAU * (3.2 * U - 2.5 * V) - phase))) ** 5
        light = np.exp(-(((U - (.5 + .22 * math.cos(phase))) / .34) ** 2
                         + ((V - .32) / .24) ** 2))
        return rgb(12 + 20 * swell + 34 * caustic + 18 * light,
                   62 + 25 * swell + 50 * swell2 + 55 * caustic + 35 * light,
                   105 + 36 * swell + 58 * swell2 + 65 * caustic + 30 * light)

    if name == "Aurora Veil":
        # Ribbons of northern light flow laterally and breathe in brightness.
        drift = .16 * math.sin(phase)
        ribbons = np.sin(TAU * (2.3 * U + .19 * np.sin(TAU * (1.1 * V) + phase) + drift))
        ribbons2 = np.sin(TAU * (3.4 * U - .13 * np.cos(TAU * V - phase) - drift))
        curtain = np.maximum(0, .58 + .42 * ribbons) ** 2
        curtain2 = np.maximum(0, .5 + .5 * ribbons2) ** 3
        band = np.exp(-((V - (.45 + .055 * np.sin(TAU * U + phase))) / .22) ** 2)
        lower = np.exp(-((V - (.66 + .035 * np.cos(TAU * U - phase))) / .15) ** 2)
        return rgb(7 + 30 * curtain * band + 22 * curtain2 * lower,
                   17 + 116 * curtain * band + 56 * curtain2 * lower + 8 * (1 - V),
                   48 + 112 * curtain * band + 74 * curtain2 * lower + 26 * (1 - V))

    if name == "Dune Light":
        # Sculpted warm dunes under a slowly travelling, cyclic sunset glow.
        sky_color = rgb(198 + 49 * (1 - V), 104 + 91 * (1 - V), 103 + 77 * (1 - V))
        ridge1 = .58 + .065 * np.sin(TAU * (1.15 * U) + .24 * math.sin(phase)) + .025 * np.sin(TAU * 3 * U)
        ridge2 = .74 + .05 * np.sin(TAU * (.9 * U + .16) - .18 * math.sin(phase)) + .018 * np.sin(TAU * 2.7 * U)
        ridge3 = .89 + .035 * np.sin(TAU * (1.4 * U + .31))
        dune1 = rgb(224 + 18 * np.cos(TAU * U + phase), 134 + 24 * np.cos(TAU * U + phase), 100 + 18 * np.cos(TAU * U + phase))
        dune2 = rgb(165 + 17 * np.cos(TAU * U - phase), 82 + 16 * np.cos(TAU * U - phase), 83 + 13 * np.cos(TAU * U - phase))
        dune3 = rgb(75 + 12 * np.cos(TAU * U + phase), 54 + 10 * np.cos(TAU * U + phase), 75 + 12 * np.cos(TAU * U + phase))
        result = sky_color
        result = blend(result, dune1, (V > ridge1).astype(np.float32))
        result = blend(result, dune2, (V > ridge2).astype(np.float32))
        result = blend(result, dune3, (V > ridge3).astype(np.float32))
        rim = np.exp(-((V - ridge1) / .012) ** 2) * (18 + 10 * math.sin(phase))
        result[..., 0] += rim
        result[..., 1] += rim * .55
        return result.clip(0, 255).astype(np.uint8)

    if name == "Leaf Garden":
        # Overlapping botanical leaves sway on closed elliptical paths.
        bg = rgb(26 + 15 * V, 59 + 38 * V, 58 + 26 * V)
        result = bg.astype(np.float32)
        for row in range(4):
            for col in range(7):
                cx = (col + .5 + .12 * math.sin(phase + row * .8 + col * .35)) / 7
                cy = (row + .5 + .045 * math.cos(phase + col * .7 + row)) / 4
                angle = (col % 2 * .46 - .23) + .12 * math.sin(phase + row + col * .45)
                dx, dy = U - cx, (V - cy) * HEIGHT / WIDTH
                ca, sa = math.cos(angle), math.sin(angle)
                along = dx * ca + dy * sa
                across = -dx * sa + dy * ca
                q = (along / .105) ** 2 + (across / .058) ** 2
                mask = np.clip((1.08 - q) * 18, 0, 1)
                vein = np.exp(-(across / .004) ** 2) * (np.abs(along) < .096)
                base = np.array((45 + 18 * (row % 2), 119 + 20 * (col % 3), 79 + 7 * (row % 3)))
                leaf = np.broadcast_to(base, result.shape).astype(np.float32).copy()
                leaf += (1 - np.clip(np.abs(across) / .06, 0, 1))[..., None] * np.array((24, 38, 19))
                leaf += vein[..., None] * np.array((35, 43, 18))
                result = blend(result, leaf, mask)
        return result.clip(0, 255).astype(np.uint8)

    if name == "Glass Garden":
        # Jewel-like tesserae catch a gentle light that completes one full orbit.
        gx, gy = U * 10, V * 7
        best = np.full((HEIGHT, WIDTH), 10_000, dtype=np.float32)
        cell = np.zeros((HEIGHT, WIDTH), dtype=np.int32)
        for iy in range(-1, 9):
            for ix in range(-1, 11):
                seed = (ix * 73856093 ^ iy * 19349663) & 0xFFFFFFFF
                rx = ((seed * 1664525 + 1013904223) & 0xFFFFFFFF) / 0xFFFFFFFF
                ry = ((seed * 22695477 + 1) & 0xFFFFFFFF) / 0xFFFFFFFF
                d = (gx - ix - (.2 + .6 * rx)) ** 2 + (gy - iy - (.2 + .6 * ry)) ** 2
                take = d < best
                best[take] = d[take]
                cell[take] = (ix * 17 + iy * 31) % 6
        palette = np.array(((238, 117, 100), (245, 194, 103), (107, 188, 166),
                            (91, 141, 190), (165, 124, 190), (241, 222, 164)), dtype=np.float32)
        color = palette[cell] * (.69 + .26 * np.maximum(0, 1 - best[..., None] * .62))
        glint = np.maximum(0, np.cos(TAU * (U * 1.4 + V * .65) - phase)) ** 12
        return (color + glint[..., None] * np.array((40, 38, 32))).clip(0, 255).astype(np.uint8)

    if name == "Deep Sea Bloom":
        # Bioluminescent anemone forms pulse while shafts of light circle above.
        dx, dy = U - .5, V - .67
        angle = np.arctan2(dy, dx)
        radius = np.sqrt(dx * dx + dy * dy)
        tentacle = np.cos(angle * 13 + .3 * np.sin(phase))
        edge = radius * (1.05 + .22 * tentacle)
        pulse = .5 + .5 * np.cos(TAU * 3 * edge - phase)
        body = np.exp(-(radius / .25) ** 4)
        shafts = np.maximum(0, np.cos(TAU * (U * 2.2 + V * .7) + phase)) ** 10 * (1 - V)
        return rgb(5 + 54 * body * pulse + 10 * shafts,
                   22 + 110 * body * pulse + 34 * shafts + 20 * V,
                   57 + 139 * body * pulse + 52 * shafts + 42 * V)

    # A radial flower with a rotating color wave; all angles close after 2π.
    dx, dy = U - .5, (V - .5) * HEIGHT / WIDTH
    angle = np.arctan2(dy, dx)
    radius = np.sqrt(dx * dx + dy * dy)
    petal = np.cos(angle * 12)
    contour = radius * (1.0 + .24 * petal)
    rings = .5 + .5 * np.cos(TAU * (contour * 11) - phase)
    center = np.exp(-(radius / .11) ** 2)
    return rgb(37 + 64 * rings + 150 * center,
               75 + 108 * rings + 111 * center,
               112 + 95 * rings + 30 * center)


SCENES = (
    ("Slow Orbit", "Psychedelic · shifting interference colours in a perfect loop"),
    ("Tidal Glass", "Ocean · rolling teal waves and cyclic caustic light"),
    ("Aurora Veil", "Aurora · flowing curtains of northern light"),
    ("Dune Light", "Desert · sculpted warm dunes beneath a moving glow"),
    ("Leaf Garden", "Botanical · layered leaves sway gently and return"),
    ("Glass Garden", "Kaleidoscopic · jewel-like glass catches the light"),
    ("Deep Sea Bloom", "Underwater · a bioluminescent bloom pulses slowly"),
    ("Geometric Bloom", "Psychedelic · a rotating radial flower and colour wave"),
)


def render(title):
    output = OUT / f"{title}.webp"
    command = [os.environ.get("FFMPEG", "ffmpeg"), "-hide_banner", "-loglevel", "error", "-y",
               "-f", "rawvideo", "-pix_fmt", "rgb24", "-s:v", f"{WIDTH}x{HEIGHT}",
               "-r", str(FPS), "-i", "pipe:0", "-frames:v", str(FRAMES), "-an",
               "-c:v", "libwebp_anim", "-lossless", "0", "-quality", "100", "-loop", "0",
               "-f", "webp", str(output)]
    process = subprocess.Popen(command, stdin=subprocess.PIPE)
    try:
        for frame in range(FRAMES):
            phase = TAU * frame / FRAMES
            process.stdin.write(scene(title, phase).tobytes())
        process.stdin.close()
    except BaseException:
        process.kill()
        process.wait()
        raise
    if process.wait() != 0:
        raise SystemExit(f"FFmpeg failed while generating {output}")
    print(f"{output.relative_to(ROOT)} ({output.stat().st_size // 1024} KiB)")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for title, _ in SCENES:
        render(title)
