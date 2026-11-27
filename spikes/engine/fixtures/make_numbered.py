"""Generate the 90-frame numbered FFV1 source from geometry using Python and FFmpeg."""

import os
from pathlib import Path
import subprocess
import tempfile


WIDTH, HEIGHT = 641, 360
RATE, COUNT = 30, 90
BACKGROUND = (24, 35, 46)
WHITE = (255, 255, 255)
SEGMENTS = ("abcdef", "bc", "abdeg", "abcdg", "bcfg",
            "acdfg", "acdefg", "abc", "abcdefg", "abcdfg")


def rect(pixels, x, y, w, h, color):
    row = bytes(color) * w
    for top in range(y, y + h):
        start = 3 * (WIDTH * top + x)
        pixels[start:start + len(row)] = row


def draw_number(pixels, index):
    shapes = {"a": (5, 0, 26, 5), "b": (31, 5, 5, 23),
              "c": (31, 33, 5, 23), "d": (5, 56, 26, 5),
              "e": (0, 33, 5, 23), "f": (0, 5, 5, 23),
              "g": (5, 28, 26, 5)}
    for position, digit in enumerate(f"{index:03d}"):
        for segment in SEGMENTS[int(digit)]:
            x, y, w, h = shapes[segment]
            rect(pixels, 476 + position * 46 + x, 26 + y, w, h, WHITE)


def frame(index):
    pixels = bytearray(bytes(BACKGROUND) * (WIDTH * HEIGHT))
    draw_number(pixels, index)
    # A moving block gives forward and reverse playback a visible direction.
    rect(pixels, 12, 266, 615, 4, (57, 78, 94))
    rect(pixels, 12 + index * 6, 252, 28, 32, (23, 211, 171))
    # Independent of text or fonts. Sample seven LSB-first cells at y=344.
    for bit in range(7):
        value = 255 if index & (1 << bit) else 0
        rect(pixels, 10 + 16 * bit, 338, 13, 13, (value, value, value))
    # An alternating right edge reveals shifted or unstripped GPU row padding.
    for y in range(HEIGHT):
        color = (31, 187, 186) if y % 2 == 0 else (217, 147, 56)
        rect(pixels, WIDTH - 1, y, 1, 1, color)
    return pixels


def main():
    target = Path(__file__).resolve().with_name("numbered.mkv")
    with tempfile.TemporaryDirectory(dir=target.parent) as temp:
        output = Path(temp) / "numbered.mkv"
        command = [
            "ffmpeg", "-nostdin", "-v", "error", "-f", "rawvideo",
            "-pixel_format", "rgb24", "-video_size", f"{WIDTH}x{HEIGHT}",
            "-framerate", str(RATE), "-i", "pipe:0", "-an", "-c:v", "ffv1",
            "-level", "3", "-g", "1", "-threads:v", "1", "-pix_fmt", "bgr0",
            "-color_primaries", "bt709", "-color_trc", "iec61966-2-1",
            "-colorspace", "rgb", "-color_range", "pc",
            "-flags:v", "+bitexact", "-fflags", "+bitexact", str(output),
        ]
        with subprocess.Popen(command, stdin=subprocess.PIPE) as proc:
            assert proc.stdin is not None
            with proc.stdin as stream:
                for index in range(COUNT):
                    stream.write(frame(index))
            if proc.wait():
                raise RuntimeError("FFmpeg failed to generate the numbered fixture")
        os.replace(output, target)


if __name__ == "__main__":
    main()
