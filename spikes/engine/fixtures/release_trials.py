"""Replay exact SDL releases into fresh native-player sessions, then revisit each target."""
import argparse
import json
import math
from pathlib import Path
import subprocess
import tempfile


def summarize(values):
    ordered = sorted(values)

    def rank(quantile):
        return round(ordered[math.ceil(quantile * len(ordered)) - 1], 2)

    return {"count": len(values), "p50_ms": rank(.5), "p95_ms": rank(.95), "max_ms": rank(1)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--range", nargs=2, type=int, required=True, metavar=("FIRST", "EXCLUSIVE_END"))
    parser.add_argument("--count", type=int, default=30)
    args = parser.parse_args()
    first, end = args.range
    if not (4 <= first < end and 2 <= args.count <= end - first):
        parser.error("choose at least two distinct release targets, starting at or after frame four")
    if args.report.exists():
        parser.error("choose a new report file")
    targets = [round(first + (end - 1 - first) * i / (args.count - 1)) for i in range(args.count)]
    results = []
    with tempfile.TemporaryDirectory(prefix="n4-release-") as scratch:
        for index, target in enumerate(targets):
            path = Path(scratch) / f"{index:02d}.json"
            command = [str(args.player.resolve()), str(args.project.resolve()), "--release-trial",
                       str(target), "--report", str(path)]
            run = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                 text=True, timeout=180)
            if run.returncode:
                raise SystemExit(f"release at frame {target} failed: {run.stdout[-3000:]}")
            info = json.loads(path.read_text())
            trial = info["release_trial"]
            if trial["landed_frame"] != info["last_displayed_frame"]:
                raise SystemExit(f"incorrect final movie frame for request {target}")
            results.append({"requested": target, "landed": trial["landed_frame"],
                            "lead": trial["lead_in_frame"],
                            "first_picture_ms": round(info["first_picture_ms"], 3),
                            "unvisited_ms": round(trial["unvisited_target_ms"], 3),
                            "revisit_ms": round(trial["revisit_ms"], 3)})
            print(f"{index+1}/{len(targets)}: frame {trial['landed_frame']}", flush=True)

    if len({row["landed"] for row in results}) != args.count:
        raise SystemExit("timeline pointer quantization collapsed requested targets; widen the interval")
    output = {
        "project": args.project.name,
        "duration_frames": info["project_frames"],
        "range": [first, end],
        "video_driver": info["video_driver"],
        "audio_driver": info["audio_driver"],
        "thumbnails": info["thumbnails_visible"],
        "boundary": trial["boundary"],
        "first_picture_boundary": (
            "From player preparation after project parsing and SDL window setup to the first "
            "movie presentation. Excludes process launch and physical display scan-out."
        ),
        "conditions": (
            "One fresh player process per target. Initial frame, audio and filmstrip ready. "
            "The quarter-distance lead-in was shown on each drag. The target was first "
            "presented on release, then revisited after the same lead-in. OS file cache "
            "was not flushed. A revisited target is not necessarily resident as a full rendered frame."
        ),
        "first_picture": summarize([r["first_picture_ms"] for r in results]),
        "cold_unvisited": summarize([r["unvisited_ms"] for r in results]),
        "warm_revisit": summarize([r["revisit_ms"] for r in results]),
        "samples": results,
    }
    with args.report.open("x") as f:
        json.dump(output, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    main()
