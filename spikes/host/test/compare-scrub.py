"""Pair the macOS development harness with N4's native SDL presentation control."""
import argparse
from bisect import bisect_right
import json
import math
import os
from pathlib import Path
import platform
import subprocess

HOST = Path(__file__).resolve().parents[1]
ENGINE = HOST.parent / "engine/target/debug"
DEFAULT_PROJECT = HOST.parent / "examples/signal-atlas"


def summarize(inputs, pictures):
    start, stop = inputs[0][0], inputs[-1][0]
    shown = [(0, pictures[0][2])]
    for at, _, frame in pictures:
        if frame != shown[-1][1]:
            shown.append((at, frame))
    times = [t for t, _ in inputs]
    draws = [t for t, _ in shown]
    boundaries = sorted(set([*times, *(t for t in draws if start < t < stop)]))
    weighted = []
    for a, b in zip(boundaries, boundaries[1:]):
        error = abs(inputs[bisect_right(times, a)-1][1] - shown[bisect_right(draws, a)-1][1])
        weighted.append((error, b-a))
    total = 0
    for lag, weight in sorted(weighted):
        total += weight
        if total >= (stop-start) * .95:
            break
    changes = [t for t in draws if start < t < stop]
    gaps = sorted((b-a)/1000 for a,b in zip([start,*changes], [*changes,stop]))
    release = next(t for t, _, f in pictures if t >= stop and f == inputs[-1][1])
    return {"pointer_events": len(inputs), "changed_pictures": len(changes),
            "hold_p95_ms": round(gaps[math.ceil(len(gaps)*.95)-1], 2),
            "time_weighted_lag_p95_frames": lag, "release_ms": round((release-stop)/1000, 2)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="New directory for raw traces and summary")
    parser.add_argument("--pairs", type=int, default=3)
    parser.add_argument("--project", type=Path, default=DEFAULT_PROJECT)
    parser.add_argument("--range", type=int, nargs=2, metavar=("START", "END"), default=[12, 84])
    args = parser.parse_args()
    project = args.project.resolve(strict=True)
    start, end = args.range
    if not 0 <= start < end:
        parser.error("choose a nonempty frame range")
    if args.pairs < 1:
        parser.error("choose at least one pair")
    electron = Path(subprocess.check_output(
        ["node", "-p", "require('electron')"], cwd=HOST, text=True).splitlines()[-1]).resolve(strict=True)
    folder = args.directory.resolve()
    folder.mkdir(parents=True)
    runs = []
    for i in range(1, args.pairs+1):
        sdl, browser = folder / f"sdl-{i}.json", folder / f"electron-{i}.json"
        subprocess.run([ENGINE / "n4-player", project, "--inspector", "--scrub-trial", "--scrub-range",
                        str(start), str(end), "--scrub-hz", "60", "--scrub-leg-ms", "460", "--report", sdl],
                       check=True, timeout=120)
        subprocess.run([electron, HOST / "test/electron-scrub.cjs", project],
                       env={**os.environ, "N4_SDL_REPORT": str(sdl), "N4_ELECTRON_REPORT": str(browser)},
                       check=True, timeout=150)
        native = json.loads(sdl.read_text())["scrub_trace"]
        host = json.loads(browser.read_text())
        assert host["scheduled"] == [f for _,f in native["inputs_us_frame"]]
        runs.append({"sdl": summarize(native["inputs_us_frame"],native["presents_us_cursor_picture"]),
                     "electron": summarize(host["inputs"],host["pictures"])})
    summary = {"project": project.name, "scrub_range": [start, end], "release_frame": native["inputs_us_frame"][-1][1],
               "system": platform.platform(), "electron_version": host["electronVersion"],
               "dpr": host["devicePixelRatio"], "profile": "debug", "runs": runs}
    (folder/"summary.json").write_text(json.dumps(summary,indent=2)+"\n")
    print(json.dumps(summary,indent=2))


if __name__ == "__main__":
    main()
