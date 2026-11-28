"""Rebuild N4's original CC0 8×4 GIF disposal and palette fixture with Python 3."""

from pathlib import Path
from struct import pack


def literal_stream(indices):
    # GIF LZW minimum code size 2. A clear (4) before each literal keeps all
    # codes three bits wide. The explicit stream makes these tiny patches portable.
    bits = 0
    length = 0
    for index in indices:
        bits |= (4 | (index << 3)) << length
        length += 6
    bits |= 5 << length  # End of image data.
    return bits.to_bytes((length + 3 + 7) // 8, "little")


def patch(left, row, delay, disposal, palette=b""):
    # Every patch is four rows tall. Index zero is transparent.
    control = b"\x21\xf9\x04" + pack("<BHBB", (disposal << 2) | 1, delay, 0, 0)
    image = b"," + pack("<HHHHB", left, 0, len(row), 4, 0x81 if palette else 0)
    lzw = literal_stream(row * 4)
    return control + image + palette + bytes([2, len(lzw)]) + lzw + b"\x00"


def generate():
    # Global palette: transparent, red, blue, yellow. The temporary frame
    # assigns green to local index one, leaving the global red untouched.
    global_palette = bytes.fromhex("000000 ff0000 0000ff ffff00")
    local_palette = bytes.fromhex("000000 00ff00 ff00ff ffffff")
    screen = b"GIF89a" + pack("<HHBBB", 8, 4, 0xf1, 0, 0) + global_palette
    loop = b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    return b"".join([
        screen, loop,
        patch(0, [1, 1, 1, 1], 4, 1),
        patch(1, [1, 0, 0, 1], 6, 3, local_palette),
        patch(6, [2, 2], 2, 2),
        patch(0, [3], 8, 1),
        b";",
    ])


if __name__ == "__main__":
    Path(__file__).with_name("restore.gif").write_bytes(generate())
