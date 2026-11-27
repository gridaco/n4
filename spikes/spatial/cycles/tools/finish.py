"""Shared finish and image utilities for the spike. Scene-linear float in, 8-bit sRGB out.

The curve is N4's film.wgsl: exposure, identity below 0.7, a continuous shoulder above."""
import json, sys
from pathlib import Path
import numpy as np
from PIL import Image


def load(path, size):
    a = np.fromfile(path, dtype="<f4")
    return a.reshape(size[1], size[0], -1)


def n4_film(rgba, exposure_ev=0.0):
    a = np.clip(rgba[..., 3:4], 0, 1) if rgba.shape[-1] == 4 else 1.0
    c = np.maximum(rgba[..., :3] / np.maximum(a, 1e-7), 0) * (2.0 ** exposure_ev)
    excess = np.maximum(c - 0.7, 0)
    return np.minimum(c, 0.7) + 0.3 * excess / (0.3 + excess)


def srgb(c):
    c = np.clip(c, 0, 1)
    return np.where(c <= 0.0031308, 12.92 * c, 1.055 * np.power(c, 1 / 2.4) - 0.055)


def finish8(rgba, exposure_ev=0.0):
    return np.round(srgb(n4_film(rgba, exposure_ev)) * 255).astype(np.uint8)


if __name__ == "__main__":
    directory, w, h = Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
    for f in sorted(directory.glob("*.rgba.f32")):
        Image.fromarray(finish8(load(f, (w, h)))).save(f.with_suffix("").with_suffix(".png"))
        print(f.name)
