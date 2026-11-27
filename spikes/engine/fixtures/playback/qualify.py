"""Evaluate the software-observable budgets from one physical full-length trial."""
import argparse
from collections import Counter
import json
import math
from pathlib import Path


def evaluate(directory):
    device = json.loads((directory / "device.json").read_text())
    load = json.loads((directory / "load.json").read_text())
    rows = device["seek_present_return"]
    if not rows:
        raise ValueError("No seeks were measured")
    def p95(name):
        return sorted(row[name] for row in rows)[math.ceil(len(rows) * .95) - 1]
    cold, warm = p95("fresh_ms"), p95("repeat_ms")
    peak = load["peak_process_tree_rss_kib"] / 1024
    jobs = Counter(e["kind"] for e in load["events"])
    end_errors = [s - n * 1600 for n, s in device["present_return_audio_frame_pairs"]]
    gates = {
        "workload_completed": load["finished"] and all(jobs[k] >= 2 for k in ("filmstrip","proxy","waveform")),
        "profile": device["project_size"] == device["drawable_pixels"] == [1920,1080]
                   and device["fps"] == [30,1] and device["display_mode"][2] >= 60
                   and device["scheduled_frames"] == 3600 and device["seconds_wall"] >= 119.9,
        "seek": len(rows) >= 30 and cold <= 250 and warm <= 100,
        "cadence": device["missed_slots"] <= 36 and device["longest_miss_run"] <= 1,
        "software_clock_alignment": bool(end_errors) and max(map(abs, end_errors)) <= 1600,
        "mixer_supply": device["app_audio_starved_samples"] == 0,
        "rss": peak <= 1536,
    }
    return {"profile_gates":gates,"p95_ms":{"decoder_cold":cold, "revisit":warm},
            "open_ms":device["indexed_project_open_ms"], "rss_peak_mib":peak,
            "missed_slots":device["missed_slots"],"longest_miss_run":device["longest_miss_run"],
            "app_audio_starved_samples":device["app_audio_starved_samples"],"completed_jobs":dict(jobs),
            "present_return_vs_callback_max_abs_samples":max(map(abs,end_errors)),
            "boundary":"OS file-cache state uncontrolled. Physical scan-out, speaker latency, and driver-level underrun counters are unmeasured."}


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("directory", type=Path)
    args = cli.parse_args()
    result = evaluate(args.directory)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if all(result["profile_gates"].values()) else 1)
