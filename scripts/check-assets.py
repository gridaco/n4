"""Verify the reviewed media inventory and keep tracked links within the checkout."""
import hashlib
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = ROOT / "docs/media.sha256"
FORMATS = set("""
.3ds .aac .aif .aiff .avi .avif .blend .bmp .csv .exr .fbx .flac .gif .glb .gltf
.hdr .heic .jpeg .jpg .m4a .m4v .mkv .mov .mp3 .mp4 .obj .ogg .otf .pdf .png
.psd .svg .tga .tif .tiff .ttc .ttf .usd .usda .usdc .usdz .wav .webm .webp
.woff .woff2 .zip
""".split())

def check_local_links(path, tracked, directories):
    """Resolve the Markdown file links used by the guides against the public tree."""
    errors = []
    for href in re.findall(r"\[[^\]\n]*\]\(([^()\s]+)\)", path.read_text()):
        target = urlsplit(href)
        if target.scheme or target.netloc or not target.path:
            continue
        resolved = (path.parent / unquote(target.path)).resolve()
        if resolved.is_relative_to(ROOT):
            name = resolved.relative_to(ROOT).as_posix()
            if name in tracked or name in directories:
                continue
        errors.append(f"{path.relative_to(ROOT)} links outside the public tree: {href}")
    return errors

def main():
    names = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0")
    tracked = set(filter(None, names))
    directories = {parent.as_posix() for name in tracked for parent in Path(name).parents}
    expected = {}
    errors = []
    for line in INVENTORY.read_text().splitlines():
        if line and not line.startswith("#"):
            digest, name = line.split("  ", 1)
            if name in expected:
                errors.append(f"duplicate inventory entry: {name}")
            expected[name] = digest

    for name in sorted(tracked):
        path = ROOT / name
        if name.startswith(".local/"):
            errors.append(f"private project material is tracked: {name}")
            continue
        if not path.is_file():
            errors.append(f"missing tracked file: {name}")
            continue
        if path.is_symlink():
            target = path.resolve()
            if (path.readlink().is_absolute() or not target.is_relative_to(ROOT)
                    or target.relative_to(ROOT).as_posix() not in tracked):
                errors.append(f"link needs a relative, tracked target: {name}")
            # The target is checked under its real path.
            continue
        if path.suffix == ".md" or path.name == "llms.txt":
            errors.extend(check_local_links(path, tracked, directories))
        with path.open("rb") as stream:
            binary = b"\0" in stream.read(8192)
        if binary or path.suffix.lower() in FORMATS or name in expected:
            actual = hashlib.sha256(path.read_bytes()).hexdigest()
            if expected.pop(name, None) != actual:
                errors.append(f"review origin and update docs/media.sha256: {name}")
    errors.extend(f"inventory entry is no longer tracked: {name}" for name in expected)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Reviewed media, documentation paths and tracked symlinks match the checkout.")
    return 0

if __name__ == "__main__":
    sys.exit(main())
