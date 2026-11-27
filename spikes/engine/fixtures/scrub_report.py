#!/usr/bin/env python3
"""Summarize an n4-player --scrub-trial report using the actual input and present clocks."""

import argparse
import json
from bisect import bisect_right
from pathlib import Path


def quantile(weighted, fraction):
    values = sorted(weighted)
    threshold = sum(weight for _, weight in values) * fraction
    cumulative = 0
    for value, weight in values:
        cumulative += weight
        if cumulative >= threshold:
            return round(value, 2)
    raise ValueError("empty measurement")


def summarize(report):
    trace = report["scrub_trace"]
    inputs = trace["inputs_us_frame"]
    presents = trace["presents_us_cursor_picture"]
    ticks = trace["ticks_per_leg"]
    if len(inputs) != 4 * ticks + 1:
        raise ValueError("incomplete pointer trace")
    input_times = [row[0] for row in inputs]
    present_times = [row[0] for row in presents]
    # Only distinct durable movie states count as picture updates.
    changes = [(b[0], b[2] - a[2]) for a, b in zip(presents, presents[1:]) if a[2] != b[2]]
    legs = []
    for leg, label in enumerate(["slow_forward", "slow_reverse", "fast_forward", "fast_reverse"]):
        a, first = inputs[ticks * leg]
        b, last = inputs[ticks * (leg + 1)]
        boundaries = sorted({a, b, *(t for t in input_times + present_times if a < t < b)})
        errors = []
        cursor_errors = []
        cursor_picture_errors = []
        stale_start = None
        stale_intervals = []
        for now, end in zip(boundaries, boundaries[1:]):
            target = inputs[bisect_right(input_times, now) - 1][1]
            _, cursor, picture = presents[bisect_right(present_times, now) - 1]
            errors.append((abs(target - picture), end - now))
            cursor_errors.append((abs(target - cursor), end - now))
            cursor_picture_errors.append((abs(cursor - picture), end - now))
            if target != picture and stale_start is None:
                stale_start = now
            elif target == picture and stale_start is not None:
                stale_intervals.append(now - stale_start)
                stale_start = None
        if stale_start is not None:
            stale_intervals.append(b - stale_start)
        updates = [t for t, _ in changes if a < t < b]
        # Include any partial freeze at either edge, even with no pictures in the leg.
        stamps = [a, *updates, b]
        gaps = [((end - now) / 1000, 1) for now, end in zip(stamps, stamps[1:])]
        requests = [a, *(t for i, (t, frame) in enumerate(inputs) if i and a < t < b
                         and frame != inputs[i - 1][1]), b]
        requested_gaps = [((t1 - t0) / 1000, 1) for t0, t1 in zip(requests, requests[1:])]
        first_correct_direction = next(
            (t for t, delta in changes if a < t < b and delta * (last - first) > 0), None
        )
        # Compare changed input targets to real presentation returns. On a
        # 60 Hz replay this is the strict one-pointer-tick freshness measure.
        changed_targets = 0
        matched_targets = 0
        for i in range(max(1, leg * ticks), (leg + 1) * ticks):
            start, target = inputs[i]
            if target == inputs[i - 1][1]:
                continue
            changed_targets += 1
            first_present = max(0, bisect_right(present_times, start) - 1)
            j = first_present
            while j < len(presents) and presents[j][0] < inputs[i + 1][0]:
                if presents[j][2] == target:
                    matched_targets += 1
                    break
                j += 1
        legs.append({
            "leg": label, "from_frame": first, "to_frame": last,
            "timeline_frames_per_wall_second": round(abs(last - first) * 1e6 / (b - a), 1),
            "updated_pictures_per_second": round(len(updates) * 1e6 / (b - a), 1),
            "changed_targets": changed_targets,
            "correct_before_next_pointer_tick": matched_targets,
            "picture_hold_ms_p50_p95_max": [quantile(gaps, q) for q in (.5, .95, 1)],
            "requested_picture_hold_ms_p95": quantile(requested_gaps, .95),
            "longest_continuous_target_mismatch_ms": round(max(stale_intervals, default=0) / 1000, 2),
            "pointer_to_picture_frames_p50_p95_max": [quantile(errors, q) for q in (.5, .95, 1)],
            "pointer_to_playhead_frames_p95": quantile(cursor_errors, .95),
            "painted_playhead_to_picture_frames_p95": quantile(cursor_picture_errors, .95),
            "first_picture_moving_in_leg_direction_ms": (
                round((first_correct_direction - a) / 1000, 2) if first_correct_direction else None
            ),
        })
    release, requested = inputs[-1]
    match = next((t for t, cursor, frame in presents if t >= release and cursor == frame == requested), None)
    if match is None and "scrub_failure" not in report:
        raise ValueError("missing exact release in successful trial")
    input_gap = [(b - a, 1) for a, b in zip(input_times, input_times[1:])]
    return {
        "release_to_matching_present_ms": round((match - release) / 1000, 2) if match is not None else None,
        "input_spacing_ms_p95_max": [round(quantile(input_gap, q) / 1000, 2) for q in (.95, 1)],
        "legs": legs,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    print(json.dumps(summarize(json.loads(args.report.read_text())), indent=2))
