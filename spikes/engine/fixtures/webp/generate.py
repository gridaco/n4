"""Generate N4's original CC0 WebP controls and independent PNG witnesses."""

from io import BytesIO
from pathlib import Path
import struct

from PIL import Image, ImageCms


OUT = Path(__file__).resolve().parent


def opaque_patch_animation():
    """Opaque subimages on a larger transparent canvas, with no VP8X alpha bit."""
    def chunk(tag, data):
        return tag + struct.pack("<I", len(data)) + data + bytes(len(data) % 2)

    def u24(number):
        return number.to_bytes(3, "little")

    frames = []
    for color, x, hold, dispose in [
        ("red", 0, 40, True), ("lime", 2, 80, False), ("white", 0, 20, False)
    ]:
        stream = BytesIO()
        Image.new("RGB", (2, 2), color).save(stream, format="WEBP", lossless=True)
        payload = stream.getvalue()[12:]
        assert payload[:4] == b"VP8L"
        header = u24(x // 2) + u24(0) + u24(1) + u24(1) + u24(hold) + bytes([2 | dispose])
        frames.append(chunk(b"ANMF", header + payload))
    canvas = chunk(b"VP8X", bytes([2, 0, 0, 0]) + u24(3) + u24(1))
    body = b"WEBP" + canvas + chunk(b"ANIM", bytes(6)) + b"".join(frames)
    return b"RIFF" + struct.pack("<I", len(body)) + body


def linear_rgb_profile():
    """A small matrix ICC profile with sRGB primaries and linear transfer curves."""
    def xyz(values):
        return b"XYZ " + bytes(4) + struct.pack(">3i", *(round(v * 65536) for v in values))

    white = (0.9642, 1.0, 0.8249)
    label = b"N4 linear RGB\0"
    tags = {
        b"desc": b"desc" + bytes(4) + struct.pack(">I", len(label)) + label + bytes(78),
        b"cprt": b"text" + bytes(4) + b"CC0 1.0\0",
        b"wtpt": xyz(white),
        b"rXYZ": xyz((0.436075, 0.222505, 0.013932)),
        b"gXYZ": xyz((0.385065, 0.716879, 0.097105)),
        b"bXYZ": xyz((0.143080, 0.060617, 0.714173)),
    }
    for channel in b"rgb":
        tags[bytes([channel]) + b"TRC"] = b"curv" + bytes(8)  # Zero entries = identity.
    header = bytearray(128)
    header[8:24] = b"\x02\x10\0\0mntrRGB XYZ "
    header[24:36] = struct.pack(">6H", 2026, 1, 1, 0, 0, 0)
    header[36:40] = b"acsp"
    header[68:80] = xyz(white)[8:]
    records = bytearray(struct.pack(">I", len(tags)))
    payload = bytearray()
    for signature, data in tags.items():
        offset = 132 + 12 * len(tags) + len(payload)
        records.extend(struct.pack(">4sII", signature, offset, len(data)))
        payload.extend(data)
        payload.extend(bytes(-len(data) % 4))
    header[:4] = struct.pack(">I", len(header) + len(records) + len(payload))
    return bytes(header + records + payload)


def main():
    decal = Image.new("RGBA", (24, 16))
    colors = [(183, 63, 125), (53, 131, 211), (231, 177, 42)]
    for y in range(decal.height):
        for x in range(decal.width):
            rgb = colors[(x // 8 + y // 8) % len(colors)]
            alpha = (0, 64, 128, 255)[(x // 6 + y // 8) % 4]
            decal.putpixel((x, y), (*rgb, alpha))
    decal.save(OUT / "decal.png")
    decal.save(OUT / "decal.webp", lossless=True, exact=True)
    for name, source in [("opaque-lossy", decal.convert("RGB")), ("alpha-lossy", decal)]:
        path = OUT / f"{name}.webp"
        source.save(path, quality=80, exact=True)
        with Image.open(path) as decoded:
            decoded.convert("RGBA").save(OUT / f"{name}.png")

    profile = linear_rgb_profile()
    decal.save(OUT / "linear-rgb.webp", lossless=True, exact=True, icc_profile=profile)
    converted = ImageCms.profileToProfile(
        decal, BytesIO(profile), ImageCms.createProfile("sRGB"), renderingIntent=1
    )
    converted.info.clear()  # Store the independent sRGB sample values without an ICC payload.
    converted.save(OUT / "linear-rgb.png")

    decal.save(OUT / "animated.webp", save_all=True, lossless=True, duration=[40, 80],
               loop=0, append_images=[decal.transpose(Image.Transpose.FLIP_LEFT_RIGHT)])
    (OUT / "patches.webp").write_bytes(opaque_patch_animation())


if __name__ == "__main__":
    main()
