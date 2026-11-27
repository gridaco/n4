"""Build a local macOS harness with its own engine, media executables and linked libraries."""
import argparse
from collections import deque
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent
ENGINE = ROOT.parent.parent / "engine"
sys.path.insert(0, str(ENGINE.parent / "spatial/openusd/tools"))
from package_macos import linked, rpaths


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL)


def copy_runtime(host, native):
    """Install each dependency at a loader-relative path, preserving USD schema discovery."""
    native.mkdir()
    roots = [(host, native / "n4-host")]
    for tool in ("ffmpeg", "ffprobe"):
        roots.append((Path(shutil.which(tool)), native / tool))
    search = [Path(p) for p in rpaths(host) if Path(p).is_absolute()]
    sdk = next((p for p in search if (p / "usd/plugInfo.json").is_file()), None)
    todo = deque(roots)
    seen = {}
    while todo:
        src, dst = todo.popleft()
        src = src.resolve(strict=True)
        if dst in seen:
            if src != seen[dst]:
                raise ValueError(f"Two libraries resolve to {dst} from {src} and {seen[dst]}")
            continue
        seen[dst] = src
        dst.parent.mkdir(exist_ok=True)
        shutil.copy2(src, dst)
        dst.chmod(0o755)
        flags = []
        if "sdl2-compat" in src.parts:
            # SDL2-compat loads SDL3 via dlopen, outside the Mach-O link table.
            sdl3 = Path(subprocess.check_output(["brew", "--prefix", "sdl3"], text=True).strip()) / "lib/libSDL3.dylib"
            todo.append((sdl3, dst.parent / "libSDL3.dylib"))
        for name in linked(src):
            if name.startswith(("/usr/lib/", "/System/Library/", "/Library/Apple/")):
                continue
            if name.startswith("@rpath/"):
                candidates = [src.parent, *search, *(Path(p) for p in rpaths(src) if p.startswith("/"))]
                dep = next((d / name[7:] for d in candidates if (d / name[7:]).exists()), None)
                if dep is None:
                    raise ValueError(f"Cannot resolve {name} from {src}")
            elif name.startswith("@loader_path/"):
                dep = src.parent / name[13:]
            else:
                dep = Path(name)
            if dep.resolve(strict=True) == src:
                continue
            kind = "sdk" if sdk and dep.resolve().is_relative_to(sdk.resolve()) else "lib"
            target = native / kind / dep.name
            flags.extend(["-change", name, "@loader_path/" + os.path.relpath(target, dst.parent)])
            todo.append((dep, target))
        if dst.suffix == ".dylib":
            flags.extend(["-id", "@rpath/" + dst.name])
        for path in rpaths(src):
            flags.extend(["-delete_rpath", path])
        if flags:
            run("install_name_tool", *flags, dst)
        run("codesign", "--force", "--sign", "-", dst)
    if sdk:
        usd = native / "sdk/usd"
        usd.mkdir()
        shutil.copy2(sdk / "usd/plugInfo.json", usd)
        for package in (sdk / "usd").iterdir():
            if package.is_dir() and (usd.parent / f"libusd_{package.name}.dylib").exists():
                shutil.copytree(package, usd / package.name)
    return len(seen)


def package(output, host):
    if output.exists():
        raise FileExistsError(f"Choose a new output directory for this build: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    # On first use Electron writes download progress before node -p prints the path.
    executable = Path(subprocess.check_output(
        ["node", "-p", "require('electron')"], cwd=ROOT, text=True).splitlines()[-1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as tmp:
        app = Path(tmp) / "N4 Viewer.app"
        run("ditto", executable.parents[2], app)
        resources = app / "Contents/Resources"
        appdir = resources / "app"
        appdir.mkdir()
        (resources / "default_app.asar").unlink(missing_ok=True)
        shutil.copytree(ROOT / "dist", appdir / "dist")
        (appdir / "package.json").write_text(json.dumps({"name": "n4-viewer", "version": "0.0.0", "main": "dist/main.cjs"}))
        count = copy_runtime(host, resources / "native")
        plist = app / "Contents/Info.plist"
        info = plistlib.loads(plist.read_bytes())
        (app / "Contents/MacOS/Electron").rename(app / "Contents/MacOS/N4Viewer")
        info.update(CFBundleExecutable="N4Viewer", CFBundleIdentifier="com.grida.n4.viewer.harness", CFBundleName="N4 Viewer", CFBundleDisplayName="N4 Viewer")
        plist.write_bytes(plistlib.dumps(info))
        run("codesign", "--force", "--sign", "-", app)
        app.rename(output)
    print(f"{output} ({count} native binaries/libraries; local development build)")


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output", type=Path, nargs="?", default=ROOT / "out/N4 Viewer.app")
    args = cli.parse_args()
    package(args.output.resolve(), Path(os.environ.get("N4_HOST_BIN", ENGINE / "target/debug/n4-host")))
