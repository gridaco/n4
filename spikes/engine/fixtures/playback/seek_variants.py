"""Benchmark fresh video decoders against a from-start pixel oracle on the 4K fixture."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import tempfile
import threading

from run import worker
import hashlib
import json
import random
import statistics
import subprocess
import time
from pathlib import Path

RGBA_BYTES = 1920 * 1080 * 4
FILTER = ("scale=1920:1080:flags=lanczos,format=yuv444p12,"
          "colorspace=iall=bt709:irange=tv:all=bt709:trc=srgb:range=pc:format=yuv444p12,"
          "scale=in_color_matrix=bt709:in_range=full:out_range=full,format=rgba")
OUTPUT = ["-fps_mode", "passthrough", "-threads:v", "1", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"]
BASE = ["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-copyts", "-noautorotate", "-filter_threads", "3"]
OPTIONS = {
    "software_4": (["-threads:v", "4"], ""),
    "software_8": (["-threads:v", "8"], ""),
    "software_12": (["-threads:v", "12"], ""),
    "software_8_small_probe": (["-threads:v", "8", "-analyzeduration", "1", "-probesize", "4096",
                                 "-fpsprobesize", "0"], ""),
    "videotoolbox": (["-hwaccel", "videotoolbox", "-hwaccel_output_format", "videotoolbox_vld"],
                     "hwdownload,format=nv12,format=yuv420p,"),
}


def read_reference(path, targets):
    # A linear decode from the start, without an input seek or the trial's trim filter.
    select = "+".join("eq(n\\,%d)" % i for i in targets)
    cmd = [*BASE, "-threads:v", "4", "-i", path, "-map", "0:v:0", "-vf", "select=" + select + "," + FILTER,
           "-frames:v", str(len(targets)), *OUTPUT]
    expected = {}
    with subprocess.Popen(cmd, stdout=subprocess.PIPE) as proc:
        for target in sorted(targets):
            raw = proc.stdout.read(RGBA_BYTES)
            if len(raw) != RGBA_BYTES:
                raise RuntimeError("incomplete linear reference")
            expected[target] = hashlib.sha256(raw).hexdigest()
        if proc.wait():
            raise RuntimeError("reference decoder failed")
    return expected


def seek(path, target, name):
    args, download = OPTIONS[name]
    # 4K workload is 30 fps with MOV clock 1/30000.
    pts = target * 1000
    start_us = pts * 1000000 // 30000 - 1
    cmd = [*BASE, *args, "-noaccurate_seek", "-seek_timestamp", "1", "-ss", f"{start_us}us",
           "-i", path, "-map", "0:v:0", "-vf", f"trim=start_pts={pts}," + download + FILTER, *OUTPUT]
    start = time.perf_counter_ns()
    with subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
        try:
            raw = proc.stdout.read(RGBA_BYTES)
            elapsed_ms = (time.perf_counter_ns() - start) / 1e6
            if len(raw) != RGBA_BYTES:
                raise RuntimeError(proc.stderr.read().decode()[:1200])
        finally:
            proc.kill()
            proc.wait()
    return elapsed_ms, hashlib.sha256(raw).hexdigest()


def run(args):
    if args.out.exists():
        raise ValueError(f"choose a new report: {args.out}")
    targets = [60 + 120 * i for i in range(30)]
    expected = read_reference(args.source, targets)
    variants = args.variants or list(OPTIONS)
    rows = []
    rng = random.Random(7231)
    background = []
    stop = threading.Event()
    with tempfile.TemporaryDirectory() as tmp, ThreadPoolExecutor(max_workers=3) as pool:
        origin = time.monotonic()
        tasks = [pool.submit(worker, kind, args.source.parent, Path(tmp), stop, origin, background)
                 for kind in ("filmstrip", "proxy", "waveform")] if args.loaded else []
        try:
            while tasks and len({event["kind"] for event in background}) != 3:
                for task in tasks:
                    if task.done():
                        task.result()
                time.sleep(0.05)
            for target in targets:
                order = variants.copy()
                rng.shuffle(order)
                for name in order:
                    latency, actual = seek(args.source, target, name)
                    rows.append({"target": target, "gop_offset": target % 75, "variant": name,
                                 "rgba_ready_ms": round(latency, 3), "match": actual == expected[target]})
        finally:
            stop.set()
            for task in tasks:
                task.result()
    summary = {}
    for name in variants:
        runs = [row for row in rows if row["variant"] == name]
        values = sorted(row["rgba_ready_ms"] for row in runs)
        summary[name] = {"p50_ms": statistics.median(values), "p95_ms": values[28], "max_ms": max(values),
                         "exact_pixels": sum(r["match"] for r in runs)}
    print(json.dumps(summary, indent=2))
    args.out.write_text(json.dumps({"sample": args.source.name, "input": "4K original, closed GOP 75",
                                    "scope": "fresh FFmpeg process to complete 1080p RGBA in CPU memory",
                                    "reference_sha256": expected, "summary": summary, "rows": rows,
                                    "background": background}, indent=2) + "\n")
    if any(stats["exact_pixels"] != len(targets) for stats in summary.values()):
        raise SystemExit("a variant differed from the sequential reference")


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("source", type=Path)
    cli.add_argument("--out", required=True, type=Path)
    cli.add_argument("--loaded", action="store_true")
    cli.add_argument("--variants", nargs="+", choices=OPTIONS)
    run(cli.parse_args())
