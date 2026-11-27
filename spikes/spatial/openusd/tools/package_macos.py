"""Package the arm64 CPU reader with the runtime closure of the provisioned OpenUSD SDK."""
import argparse
import json
import shutil
import subprocess
import tempfile
from collections import deque
from pathlib import Path


def run(*args):
    return subprocess.check_output(args, text=True)


def linked(path):
    return [line.split(" (compatibility version", 1)[0].strip()
            for line in run("otool", "-L", path).splitlines()[1:]]


def rpaths(path):
    lines = run("otool", "-l", path).splitlines()
    return [lines[i + 2].strip().split(" (offset ", 1)[0].removeprefix("path ")
            for i, line in enumerate(lines) if line.strip() == "cmd LC_RPATH"]


def resolve(name, parent, sdk):
    if name.startswith(("/System/Library/", "/usr/lib/", "/Library/Apple/")):
        return None
    tail = name.split("/", 1)[-1]
    if name.startswith("@loader_path/"):
        path = parent.parent / tail
    elif name.startswith("@rpath/"):
        path = sdk / "lib" / tail
    else:
        path = Path(name)
    if not path.is_file() or not path.resolve().is_relative_to(sdk):
        raise ValueError(f"Unexpected external dependency on {name} from {parent}")
    return path


def assemble(binary, sdk, destination):
    sdk = sdk.resolve(strict=True)
    lib = destination / "lib"
    (destination / "bin").mkdir()
    lib.mkdir()
    todo = deque([(binary.resolve(strict=True), destination / "bin" / "inspect_stage")])
    visited = {}
    libraries = set()
    while todo:
        source, target = todo.popleft()
        if target in visited:
            if source.resolve() != visited[target]:
                raise ValueError(f"Conflicting SDK libraries named {target.name}")
            continue
        visited[target] = source.resolve()
        shutil.copy2(source, target)
        target.chmod(0o755)
        flags = []
        if target.parent == lib:
            flags += ["-id", f"@rpath/{target.name}"]
            libraries.add(target.name)
        for name in linked(source):
            dep = resolve(name, source, sdk)
            if dep is None or dep.resolve() == source.resolve():
                continue
            relative = "@loader_path/" if target.parent == lib else "@loader_path/../lib/"
            flags += ["-change", name, relative + dep.name]
            todo.append((dep, lib / dep.name))
        for path in rpaths(source):
            flags += ["-delete_rpath", path]
        if flags:
            run("install_name_tool", *flags, target)
        run("codesign", "--force", "--sign", "-", target)

    # USD discovers concrete schema types from plugInfo and its generatedSchema layer.
    plugin_root = lib / "usd"
    plugin_root.mkdir()
    source_plugins = sdk / "lib/usd"
    shutil.copy2(source_plugins / "plugInfo.json", plugin_root)
    for child in sorted(source_plugins.iterdir()):
        if f"libusd_{child.name}.dylib" in libraries and child.is_dir():
            shutil.copytree(child, plugin_root / child.name)

    # Identify source packages, and include their upstream license notices when available.
    package_records = []
    for record in sorted((sdk / "conda-meta").glob("*.json")):
        meta = json.loads(record.read_text())
        files = set(meta.get("files", []))
        if not any(f"lib/{name}" in files for name in libraries):
            continue
        package_records.append({key: meta.get(key) for key in ("name", "version", "build", "license")})
        cache = (meta.get("link") or {}).get("source")
        license_dir = Path(cache) / "info/licenses" if cache else None
        if license_dir and license_dir.is_dir():
            shutil.copytree(license_dir, destination / "licenses" / meta["name"])
    (destination / "packages.json").write_text(json.dumps(package_records, indent=2) + "\n")
    tools = Path(__file__).resolve().parent
    (destination / "checks").mkdir()
    for name in ("hinge.usda", "seam.usdz"):
        shutil.copy2(tools.parent / "tests/data" / name, destination / "checks" / name)
    shutil.copy2(tools.parents[3] / "LICENSES/CC0-1.0.txt", destination / "checks/LICENSE.txt")
    check = destination / "check-reader.sh"
    shutil.copy2(tools / check.name, check)
    check.chmod(0o755)
    return len(libraries), len(package_records)


def package(binary, sdk, destination):
    if destination.exists():
        raise FileExistsError(destination)
    destination.parent.mkdir(exist_ok=True, parents=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as scratch:
        staging = Path(scratch) / "bundle"
        staging.mkdir()
        libraries, packages = assemble(binary, sdk, staging)
        staging.rename(destination)
    print(f"{libraries} runtime libraries; {packages} source packages; {destination}")


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("binary", type=Path)
    cli.add_argument("sdk", type=Path)
    cli.add_argument("destination", type=Path, help="new output directory")
    args = cli.parse_args()
    package(args.binary, args.sdk, args.destination)
