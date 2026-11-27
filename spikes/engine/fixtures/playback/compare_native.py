"""Paired native measurements on the Mac for a screen recording and presenter edit."""
import gzip
import hashlib
import json
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scrub_report import summarize


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def drag(player, project, destination, frame_count):
    with tempfile.TemporaryDirectory() as tmp:
        report = Path(tmp) / "scrub.json"
        subprocess.run([player, project, "--scrub-trial", "--report", report],
                       check=True, timeout=150)
        raw = json.loads(report.read_text())
        with report.open("rb") as inp, gzip.open(destination, "wb") as out:
            shutil.copyfileobj(inp, out)
    assert raw["project_frames"] == frame_count and raw["thumbnails_visible"] == 20
    target = raw["scrub_trace"]["inputs_us_frame"][-1][1]
    assert target == raw["last_requested_frame"] == raw["last_displayed_frame"]
    return {**summarize(raw), "release_target": target, "first_picture_ms": raw["first_picture_ms"]}


def playback(probe, project, path, seconds, start, end):
    subprocess.run([probe, project, "--seconds", str(seconds), "--report", path],
                   check=True, timeout=150)
    report = json.loads(path.read_text())
    num, den = report["fps"]
    on_time = {0}  # The device probe primes frame zero before it starts the audio clock.
    for frame, sample in report["present_return_audio_frame_pairs"]:
        if sample * num // (48_000 * den) == frame:
            on_time.add(frame)
    assert seconds * num // den == report["scheduled_frames"] == len(on_time) + report["missed_slots"]
    return {key: report[key] for key in ("scheduled_frames", "missed_slots", "longest_miss_run",
            "app_audio_starved_samples", "display_mode", "drawable_pixels")} | {
            "overlap_slots": end - start, "missed_in_overlap_window": sum(
                frame not in on_time for frame in range(start, end))}


def measure_graphics_pair(projects, profile, output, binary_dir):
    """Alternate a graphics edit and its control, then play each entire cut."""
    baseline, variant = projects
    frames = sum(shot["duration"] for shot in profile["shots"])
    traces = output / "traces"
    traces.mkdir(parents=True, exist_ok=False)
    result = {
        "device": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
        "size": profile["size"], "fps": profile["fps"], "frames": frames,
        "binaries_sha256": {name: digest(binary_dir / name) for name in ("n4-device", "n4-player")},
        "boundary": "SDL present return", "scrub": {name: [] for name in projects}, "playback": {},
    }
    orders = [(baseline, variant), (variant, baseline), (baseline, variant)]
    for trial, order in enumerate(orders, 1):
        for name in order:
            raw = traces / f"{name}-drag-{trial}.json.gz"
            print(f"Scrub {trial}, {name}", flush=True)
            result["scrub"][name].append(drag(binary_dir / "n4-player", projects[name], raw, frames))
    for name in (variant, baseline):
        raw = traces / f"{name}-play.json"
        subprocess.run([binary_dir / "n4-device", projects[name], "--whole-project",
                        "--report", raw], check=True, timeout=90)
        report = json.loads(raw.read_text())
        assert report["scheduled_frames"] == frames
        result["playback"][name] = {key: report[key] for key in
            ("scheduled_frames", "missed_slots", "longest_miss_run", "app_audio_starved_samples",
             "drawable_pixels", "display_mode")}
    (output / "performance.json").write_text(json.dumps(result, indent=2) + "\n")


def measure_pair(projects, control, clip, output, binary_dir):
    baseline, variant = projects
    project_dir = projects[baseline].parent
    num, den = control["fps"]
    frames = sum(shot["duration"] for shot in control["shots"])
    seconds, remainder = divmod(frames * den, num)
    assert remainder == 0, "device probe expects an integer playback duration"
    start, end = clip["start"], clip["start"] + clip["duration"]
    assert 0 < start < end < frames
    traces = output / "traces"
    traces.mkdir(parents=True, exist_ok=False)
    binaries = {name: binary_dir / name for name in ("n4-player", "n4-device")}
    result = {
        "device": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
        "macos": platform.mac_ver()[0], "profile": binary_dir.name,
        "binaries_sha256": {n: digest(p) for n, p in binaries.items()},
        "projects_sha256": {n: digest(p) for n, p in projects.items()},
        "sources_sha256": {Path(s).name: digest(project_dir / s)
                           for s in [control["shots"][0]["source"], clip["source"]]},
        "size": control["size"], "fps": [num, den], "inset_size": clip["size"],
        "frames": frames, "overlap": [start, end], "scrub": {baseline: [], variant: []},
        "playback": {},
    }
    def save():
        (output / "performance.json").write_text(json.dumps(result, indent=2) + "\n")

    for i, order in enumerate(((baseline, variant), (variant, baseline), (baseline, variant)), 1):
        for name in order:
            print(f"Native drag {i}/3, {name}", flush=True)
            result["scrub"][name].append(drag(binaries["n4-player"], projects[name],
                                          traces / f"{name}-drag-{i}.json.gz", frames))
            save()
    for name in (variant, baseline):
        print(f"Audio-clocked playback, {name}", flush=True)
        result["playback"][name] = playback(binaries["n4-device"], projects[name],
            traces / f"{name}-play.json", seconds, start, end)
        save()
    assert result["binaries_sha256"] == {n: digest(p) for n, p in binaries.items()}
    return result
