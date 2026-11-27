"""Rebuild Signal Atlas source media. Python 3 standard library, no external assets."""
import array
import math
from pathlib import Path
import struct
import sys
import wave
import zlib

ASSETS = Path(__file__).resolve().parent / "assets"
TAU = math.tau


def relief():
    width, height = 568, 380
    raw = bytearray()
    for y in range(height):
        raw.append(0)  # PNG scanline filter
        for x in range(width):
            u, v = (x - 320) / 160, (y - 160) / 142
            u += .30 * math.sin(v * 2.4) + .12 * math.cos(v * 5)
            v += .17 * math.sin(u * 2.1)
            depth = math.hypot(u, v) + .12 * math.sin(u * 3 - v * 2)
            level = depth * 16
            contour = max(0, 1 - abs(level - round(level)) * 5)
            if round(level) % 4 == 0:
                contour *= 1.6
            light = max(0, min(1, .85 - depth * .28))
            grain = ((x * 101 + y * 199 + x * y * 7) % 17 - 8) / 4
            grid = 9 if x % 48 == 0 or y % 48 == 0 else 0
            raw.extend(min(255, max(0, round(dark + light * ramp + contour * ink + grid + grain)))
                       for dark, ramp, ink in ((10, 14, 63), (37, 42, 62), (50, 50, 52)))

    def chunk(tag, payload):
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload))

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    (ASSETS / "relief.png").write_bytes(data + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def echo():
    grid = [f"M{x} 0 V242" for x in range(0, 886, 48)] + [f"M0 {y} H884" for y in range(0, 244, 32)]
    curves = []
    for row in range(35):
        points = []
        for x in range(0, 887, 6):
            y = (30 + row * 4.5 + 14 * math.sin(x / 44 + row * .06)
                 + (48 - row * .8) * math.sin(x / 102 + math.sin(x / 76)))
            points.append(f"{x} {y:.1f}")
        curves.append("M" + " L".join(points))
    (ASSETS / "echo.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="888" height="244">\n'
        + f'  <path fill="none" stroke="#36585e" stroke-width="1" d="{" ".join(grid)}"/>\n'
        + f'  <path fill="none" stroke="#719d99" stroke-width="1" d="{" ".join(curves)}"/>\n</svg>\n'
    )


def pings():
    rate = 48000
    samples = array.array("h", [0]) * (rate * 8)
    for start, pitch in ((0, 196), (.75, 293.66), (2.66, 246.94), (3.5, 369.99), (5.33, 196), (6.07, 392)):
        start_index = round(start * rate)
        for i in range(min(round(1.8 * rate), len(samples) - start_index)):
            t = i / rate
            envelope = (1 - math.exp(-65 * t)) * math.exp(-3.9 * t)
            signal = math.sin(TAU * pitch * t + .8 * math.exp(-4 * t) * math.sin(TAU * pitch * 2 * t))
            samples[start_index + i] += round(5800 * signal * envelope)
    if sys.byteorder != "little":
        samples.byteswap()
    with wave.open(str(ASSETS / "pings.wav"), "wb") as sound:
        sound.setparams((1, 2, rate, 0, "NONE", "not compressed"))
        sound.writeframes(samples.tobytes())


if __name__ == "__main__":
    relief()
    echo()
    pings()
