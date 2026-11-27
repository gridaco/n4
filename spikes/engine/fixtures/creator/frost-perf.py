"""Compare full-quality backdrop rendering on a Mac with Metal, SDL and FFmpeg."""
import argparse
import copy
import gzip
import hashlib
import json
import math
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from frost_project import project

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
from scrub_report import summarize
from session_replay import session

FRAMES = 180
DRAG_LOOPS = 5
REPEATS = 3
BLOCK_SAMPLES = 12
PROFILES = {
    "demo": (1, None, 8),
    "caption": (2, None, 16),
    "large": (2, (1760, 930), 16),
}


def dump(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n")


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def materialize(root):
    # Materialize genuine 1080p H.264 packets so the measured decode does no scaling.
    plate = root / "take-b-1080.mp4"
    subprocess.run([
        "ffmpeg", "-v", "error", "-i", HERE / "take-b.mp4", "-an", "-vf", "scale=1920:1080",
        "-c:v", "libx264", "-threads", "3", "-crf", "16", "-g", "60", "-bf", "3",
        "-pix_fmt", "yuv420p", "-color_range", "tv", "-colorspace", "bt709",
        "-color_trc", "bt709", "-color_primaries", "bt709", plate,
    ], check=True)

    def resolve(obj):
        if isinstance(obj, list):
            for item in obj:
                resolve(item)
        if isinstance(obj, dict):
            for key, val in obj.items():
                if key in ("source", "font", "shader") and isinstance(val, str):
                    obj[key] = str((HERE / val).resolve())
                else:
                    resolve(val)
    cases = {}
    for family, (scale, size, strength) in PROFILES.items():
        for enabled in (False, True):
            edit = project(scale=scale, card_size=size, sigma_px=strength if enabled else 0)
            resolve(edit)
            if scale == 2:
                edit["shots"][0]["source"] = str(plate)
            name = f"{family}-{'on' if enabled else 'off'}"
            cases[name] = root / (name + ".json")
            dump(cases[name], edit)
            if scale == 2:
                # Keep the native player's standard 10s gesture at ~1x and ~10x.
                # A single 6s cut makes its slow leg request only 5.6 pictures/s.
                drag = copy.deepcopy(edit)
                drag["shots"] *= DRAG_LOOPS
                drag["audio"] = [
                    dict(clip, start_sample=clip["start_sample"] + loop * 288_000)
                    for loop in range(DRAG_LOOPS) for clip in edit["audio"]
                ]
                dump(root / (name + "-drag.json"), drag)
    return cases, fingerprint(plate)


def parked_pair(binary, path, strength, reverse):
    """Paired samples share a decoder, GPU device and compiled pipelines."""
    values = {False: [], True: []}
    counters = {}
    order = (True, False, False, True) if reverse else (False, True, True, False)
    with session(binary, path) as send:
        revision = 0
        for enabled in order:
            revision = send("set", at=revision, path="/graphics/2/content/filter/sigma_px",
                            value=strength if enabled else 0)["revision"]
            for _ in range(5):
                send("inspect", frame=96)
            for _ in range(BLOCK_SAMPLES):
                start = time.perf_counter_ns()
                trace = send("inspect", frame=96)["result"]
                values[enabled].append((time.perf_counter_ns() - start) / 1_000_000)
                assert trace["reused_decode"] and trace["request"]["timeline_frame"] == 96
                assert trace["graphics"]["graphic_uploads"] == 0
            counters[enabled] = trace["graphics"]
            assert counters[enabled]["backdrop_passes"] == (3 if enabled else 0)
    results = {}
    for enabled, durations in values.items():
        results["on" if enabled else "off"] = {
            "samples_ms": [round(d, 4) for d in durations], "graphics": counters[enabled],
            "median_ms": round(statistics.median(durations), 3),
            "p95_ms": round(sorted(durations)[math.ceil(.95 * len(durations)) - 1], 3),
        }
    return results


def windowed(binary_dir, scene, raw, round_id):
    # Raw pointer/present events let the reader audit the worst frozen interval.
    stem = f"{scene.stem}-{round_id}"
    scrub_path = raw / (stem + "-scrub.json")
    drag = scene.with_name(scene.stem + "-drag.json")
    run = subprocess.run([binary_dir / "n4-player", drag, "--scrub-trial", "--report", scrub_path],
                         timeout=90)
    if not scrub_path.is_file():
        run.check_returncode()
    report = json.loads(scrub_path.read_text())
    if run.returncode == 0:
        assert report["last_requested_frame"] == report["last_displayed_frame"]
    elif "scrub_failure" not in report:
        run.check_returncode()
    assert report["thumbnails_visible"] == 20 and report["project_frames"] == FRAMES * DRAG_LOOPS
    scrub = summarize(report)
    scrub["release_target"] = report["scrub_trace"]["inputs_us_frame"][-1][1]
    scrub["last_movie_frame"] = report["last_displayed_frame"]
    if "scrub_failure" in report:
        scrub["failure"] = report["scrub_failure"]
    # Compress only our own freshly produced trace; device receipts are already small.
    with gzip.open(str(scrub_path) + ".gz", "wb") as compressed:
        compressed.write(scrub_path.read_bytes())
    scrub_path.unlink()
    play_path = raw / (stem + "-playback.json")
    subprocess.run([binary_dir / "n4-device", scene, "--seconds", "6", "--report", play_path],
                   check=True, timeout=90)
    play = json.loads(play_path.read_text())
    assert play["project_size"] == [1920, 1080] and play["scheduled_frames"] == FRAMES
    return {"scrub": scrub, "playback": {key: play[key] for key in
            ("scheduled_frames", "missed_slots", "longest_miss_run", "app_audio_starved_samples")}}


def environment(binary_dir, plate_hash):
    return {
        "cpu": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
        "macos": platform.mac_ver()[0],
        "binary_sha256": {name: fingerprint(binary_dir / name) for name in ("n4", "n4-player", "n4-device")},
        "full_quality": True,
        "profile_directory": binary_dir.name,
        "derived_1080p_source_sha256": plate_hash,
        "source_sha256": fingerprint(HERE / "take-b.mp4"),
        "parked_frame": 96,
        "scrub_project_frames": FRAMES * DRAG_LOOPS,
        "warmups_per_parked_block": 5,
        "samples_per_parked_block": BLOCK_SAMPLES,
        "parked_blocks_per_condition_per_round": 2,
        "rounds_per_profile": REPEATS,
    }


def compare(rounds):
    """Report medians across repetitions; retain the individual runs above them."""
    result = {}
    for family in PROFILES:
        pairs = [(r[family + "-off"], r[family + "-on"]) for r in rounds]
        med = statistics.median
        result[family] = {
            "paired_parked_delta_median_ms": round(med(
                on["parked"]["median_ms"] - off["parked"]["median_ms"] for off, on in pairs
            ), 3),
        }
        for index, status in enumerate(("off", "on")):
            rows = [pair[index] for pair in pairs]
            numbers = {"parked_median_ms": med(r["parked"]["median_ms"] for r in rows),
                       "parked_p95_ms": med(r["parked"]["p95_ms"] for r in rows)}
            result[family][status] = numbers
            if family == "demo":
                continue
            numbers["release_ms"] = [r["scrub"]["release_to_matching_present_ms"] for r in rows]
            numbers["missed_slots"] = [r["playback"]["missed_slots"] for r in rows]
            for k, leg in enumerate(rows[0]["scrub"]["legs"]):
                trials = [r["scrub"]["legs"][k] for r in rows]
                numbers[leg["leg"]] = {
                    "p95_hold_ms": med(t["picture_hold_ms_p50_p95_max"][1] for t in trials),
                    "worst_hold_ms": max(t["picture_hold_ms_p50_p95_max"][2] for t in trials),
                    "updates_per_second": med(t["updated_pictures_per_second"] for t in trials),
                    "p95_error_frames": med(t["pointer_to_picture_frames_p50_p95_max"][1] for t in trials),
                }
    return result


def run(binary_dir, output):
    raw = output / "perf"
    raw.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="n4-frost-bench-") as temp:
        root = Path(temp)
        # Keep all repetitions on the same executables if Cargo updates its target.
        frozen = root / binary_dir.name
        frozen.mkdir()
        for name in ("n4", "n4-device", "n4-player"):
            try:
                os.link(binary_dir / name, frozen / name)
            except OSError:
                shutil.copy2(binary_dir / name, frozen / name)
        cases, plate_hash = materialize(root)
        result = {"environment": environment(frozen, plate_hash), "rounds": []}
        # Interleave on/off and reverse their order in the middle repetition.
        for i in range(REPEATS):
            order = list(cases) if i != 1 else list(reversed(cases))
            current = {}
            result["rounds"].append(current)
            for family, (_, _, sigma) in PROFILES.items():
                measured = parked_pair(frozen / "n4", cases[family + "-on"], sigma, i == 1)
                for status, row in measured.items():
                    current[f"{family}-{status}"] = {"parked": row}
            for name in order:
                if not name.startswith("demo-"):
                    print(f"Run {i+1}/{REPEATS} {name}", flush=True)
                    current[name].update(windowed(frozen, cases[name], raw, i+1))
                    dump(output / "performance.json", result)
        result["comparisons"] = compare(result["rounds"])
        dump(output / "performance.json", result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary_dir", type=Path, help="binaries with device-probe,native-decode")
    parser.add_argument("output", type=Path, help="parent of a new perf/ trace directory")
    args = parser.parse_args()
    run(args.binary_dir.resolve(), args.output.resolve())
