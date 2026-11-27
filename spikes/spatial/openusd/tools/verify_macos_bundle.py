"""Exercise a relocated macOS reader with development files, file writes and network denied."""
import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
from pathlib import Path


def invoke(command, **kwargs):
    return subprocess.run(command, text=True, capture_output=True, **kwargs)


def inspect_links(bundle):
    """Resolve the entire packaged Mach-O closure without consulting external rpaths."""
    lib = bundle / "lib"
    files = [bundle / "bin/inspect_stage", *sorted(lib.glob("*.dylib"))]
    for file in files:
        subprocess.run(["codesign", "--verify", "--strict", file], check=True)
        commands = subprocess.check_output(["otool", "-l", file], text=True)
        if any(line.strip() == "cmd LC_RPATH" for line in commands.splitlines()):
            raise ValueError(f"{file.name} retains an external runtime search path")
        loads = subprocess.check_output(["otool", "-L", file], text=True).splitlines()[1:]
        for index, line in enumerate(loads):
            name = line.strip().split(" (compatibility version", 1)[0]
            if file.parent == lib and index == 0 and name == f"@rpath/{file.name}":
                continue  # LC_ID_DYLIB is a name, not an outgoing dependency.
            if name.startswith(("/usr/lib/", "/System/Library/", "/Library/Apple/")):
                continue
            if not name.startswith("@loader_path/"):
                raise ValueError(f"{file.name} still links outside its bundle: {name}")
            target = (file.parent / name.removeprefix("@loader_path/")).resolve(strict=True)
            if target.parent != lib:
                raise ValueError(f"{file.name} reaches outside its library directory: {target}")
    return len(files) - 1


def qualify(bundle, stage_inputs, sdk, receipt):
    if platform.system() != "Darwin":
        raise SystemExit("macOS is required")
    # Deny /private/tmp except the moved package, including other builds left there.
    with tempfile.TemporaryDirectory(prefix="nothing-usd-", dir="/private/tmp") as scratch:
        scratch = Path(scratch).resolve()
        root = scratch / "moved runtime"
        shutil.copytree(bundle, root)
        count = inspect_links(root)
        fixtures = root / "fixtures"
        fixtures.mkdir()
        (fixtures / "leaf.usda").write_text(
            '#usda 1.0\ndef Mesh "Part" {\n uniform token subdivisionScheme="none"\n'
            ' point3f[] points=[(0,0,0),(1,0,0),(0,1,0)]\n int[] faceVertexCounts=[3]\n'
            ' int[] faceVertexIndices=[0,1,2]\n}\n')
        (fixtures / "assembly.usda").write_text(
            '#usda 1.0\ndef Xform "Product" {\n def "Housing" '
            '(prepend references=@leaf.usda@</Part>) {}\n}\n')
        cases = [(fixtures / "assembly.usda", "/Product", (1, 2, 1, 0))]
        for file, prim, triangles, parts, slots, images in stage_inputs:
            sample = fixtures / Path(file).name
            if sample.exists():
                raise ValueError(f"Repeated fixture name {sample.name}")
            shutil.copy2(file, sample)
            cases.append((sample, prim, tuple(map(int, (triangles, parts, slots, images)))))

        forbidden = scratch / "unbundled.txt"
        forbidden.write_text("negative control")
        denied = ["/Users", "/opt", "/usr/local", "/Applications", "/Library/Developer",
                  "/Library/Frameworks", "/Library/Application Support", "/private/var/root",
                  "/private/tmp", "/System/Volumes/Data/Users", "/System/Volumes/Data/opt",
                  "/System/Volumes/Data/usr/local", "/System/Volumes/Data/private/tmp"]
        if sdk:
            sdk = sdk.resolve(strict=True)
            denied.append(str(sdk))
        rules = " ".join(f"(subpath {json.dumps(p)})" for p in denied)
        profile = (f"(version 1) (allow default) (deny network*) (deny file-write*) "
                   f"(deny file-read* {rules}) (allow file-read* (subpath {json.dumps(str(root))}))")
        sandbox = ["/usr/bin/sandbox-exec", "-p", profile]
        env = {"HOME": str(root), "TMPDIR": str(root), "PATH": "/usr/bin:/bin", "LC_ALL": "C"}
        probes = [forbidden]
        if sdk:
            probes.append(sdk / "include/pxr/pxr.h")
        for probe in probes:
            if not probe.is_file():
                raise ValueError(f"Missing negative control: {probe}")
            check = invoke([*sandbox, "/bin/cat", str(probe)], cwd=root, env=env)
            if not check.returncode or str(probe) not in check.stderr or "Operation not permitted" not in check.stderr:
                raise RuntimeError(f"File denial was not demonstrated: {check.stderr}")

        results = []
        for path, prim, expected in cases:
            run = invoke([*sandbox, root / "bin/inspect_stage", path, prim], cwd=root, env=env)
            if run.returncode:
                raise RuntimeError(f"Relocated reader failed on {path.name}: {run.stderr}")
            line = run.stdout.splitlines()[0]
            match = re.fullmatch(r"(\d+) triangles, (\d+) rigid parts, (\d+) material slots, (\d+) images", line)
            if not match or tuple(map(int, match.groups())) != expected:
                raise ValueError(f"{path.name} {prim}: expected {expected}, got {line}")
            results.append(dict(stage=path.name, prim=prim, triangles=expected[0],
                                parts=expected[1], material_slots=expected[2], images=expected[3]))
        data = dict(system=platform.mac_ver()[0], architecture=platform.machine(), runtime_dylibs=count,
                    code_signatures_verified=True, unbundled_file_denied=True,
                    development_sdk_read_denied=bool(sdk), cases=results)
        output = json.dumps(data, indent=2) + "\n"
        if receipt:
            receipt.write_text(output)
        print(output, end="")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--sdk", type=Path, help="also prove the installed SDK header is unreadable")
    parser.add_argument("--stage", nargs=6, action="append", default=[],
                        metavar=("FILE", "PRIM", "TRIANGLES", "PARTS", "SLOTS", "IMAGES"))
    parser.add_argument("--receipt", type=Path)
    args = parser.parse_args()
    qualify(args.bundle, args.stage, args.sdk, args.receipt)
