"""Inputs for the Field Unit reflection close-up used to qualify the Cycles bridge.

Writes display artwork frames, the nameplate and shot.json into an output directory (default
.local/cycles/shots/field-unit-reflection at the repository root). The model and font come from the
Field Unit example; the oscilloscope drawing is that example's own generator.

    python3 spikes/spatial/cycles/shots/field-unit-reflection/make_inputs.py [OUT]
"""
import json, math, os, sys
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFont

REPO = Path(__file__).resolve().parents[5]
EXAMPLE = REPO / "spikes/examples/field-unit"
ROOT = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else REPO / ".local/cycles/shots/field-unit-reflection"
FONT = str(EXAMPLE / "assets/N4GridSans.ttf")
sys.path.insert(0, str(EXAMPLE / "author"))
from media_sources import signal  # noqa: E402  the example's own display generator

FPS = 24
FRAMES = 51


def artwork(directory, word, frames):
    directory.mkdir(parents=True, exist_ok=True)
    for frame in frames:
        im = signal(frame)
        draw = ImageDraw.Draw(im)
        draw.text((26, 7), word, font=ImageFont.truetype(FONT, 42), fill="#f1eacb")
        draw.text((28, 65), "CH 01   /   INPUT MONITOR", font=ImageFont.truetype(FONT, 20), fill="#c1cabc")
        im.convert("RGBA").save(directory / f"signal-{frame:03}.png")


def plate():
    im = Image.new("RGB", (440, 112), "#b4b09b")
    draw = ImageDraw.Draw(im)
    draw.text((12, 9), "R-01", font=ImageFont.truetype(FONT, 70), fill="#202624")
    for x, w in [(287, 7), (301, 3), (310, 12), (330, 6), (344, 3), (354, 9), (373, 4), (389, 13), (412, 5)]:
        draw.rectangle((x, 22, x + w - 1, 87), fill="#202624")
    im.convert("RGBA").save(ROOT / "artwork/plate.png")


def sub(a, b): return [a[i] - b[i] for i in range(3)]
def cross(a, b): return [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]
def unit(a):
    l = math.sqrt(sum(x * x for x in a)); return [x / l for x in a]


def rect(center, aim, width, height, radiance):
    """A rectangle facing `aim`. u is horizontal; cross(u, v) is the emission direction."""
    n = unit(sub(aim, center))
    u = unit(cross(n, [0, 1, 0]))
    v = cross(n, u)                      # cross(u, v) == n
    return {"center": center, "u": [x * width for x in u], "v": [x * height for x in v],
            "radiance": radiance, "width": width, "height": height, "aim": aim}


def look(position, target):
    f = unit(sub(target, position))
    pitch = math.degrees(math.asin(f[1]))
    yaw = math.degrees(math.atan2(-f[0], -f[2]))
    return [pitch, yaw, 0.0]


def shot():
    cam = [0.150, 0.118, 0.215]
    target = [-0.005, 0.048, -0.012]
    spec = {
        "name": "field-unit-reflection",
        "fps": FPS, "frames": FRAMES, "size": [1920, 1080],
        "asset": os.path.relpath(EXAMPLE / "assets/field.usdc", ROOT), "product": "/World/Recorder", "ground": "/World/Studio",
        "hinge": {"part": "/World/Recorder/Hinge", "axis": 0, "keys": [[0, 58], [38, 14], [50, 14]]},
        "camera": {"position": cam, "target": target, "rotation": look(cam, target), "fov_y": 27.0},
        "display": {"slot": "/World/_materials/Display", "emission": 1.0, "glow": 4.0},
        "nameplate": {"slot": "/World/_materials/Nameplate", "image": "artwork/plate.png"},
        "lights": {
            "rects": [
                {"name": "key", **rect([-0.36, 0.52, 0.34], [0, 0.05, 0], 0.50, 0.30, [11.0, 9.7, 7.9])},
                {"name": "strip", **rect([0.12, 0.34, -0.40], [0, 0.08, 0], 0.52, 0.05, [14.0, 12.6, 10.5])},
                {"name": "card", **rect([0.34, 0.20, 0.22], [0, 0.06, 0], 0.50, 0.20, [1.3, 1.5, 1.75])},
            ],
            "background": [0.045, 0.050, 0.056],
        },
        "finish": {"exposure_ev": 0.0, "curve": "n4-film"},
        "hold": [39, 51],
    }
    (ROOT / "shot.json").write_text(json.dumps(spec, indent=2) + "\n")


if __name__ == "__main__":
    (ROOT / "artwork").mkdir(parents=True, exist_ok=True)
    artwork(ROOT / "artwork/base", "FIELD UNIT", range(FRAMES + 4))
    artwork(ROOT / "artwork/word", "FIELD MARK", range(FRAMES + 4))
    plate()
    shot()
    print(f"wrote artwork and shot.json to {ROOT}")
