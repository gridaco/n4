"""Measure ten cycles on one native playback/filmstrip session without a window."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time

from run import rss_kib, run_job


def measure(args):
    root = args.project.resolve().parent
    output = args.out.resolve()
    if output.exists():
        raise ValueError(f"Choose a new report: {output}")
    origin = time.monotonic()
    samples = []
    stop = threading.Event()

    def monitor():
        while not stop.is_set():
            samples.append({"s": round(time.monotonic() - origin, 3), "rss_kib": rss_kib()})
            stop.wait(0.2)

    cycles = []
    with ThreadPoolExecutor(max_workers=4) as pool, tempfile.TemporaryDirectory() as tmp:
        watch = pool.submit(monitor)
        work = Path(tmp)
        child = subprocess.Popen([args.probe.resolve(), args.project.resolve()], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, text=True, bufsize=1)
        replies = queue.Queue()

        def read_reports():
            try:
                for line in child.stdout:
                    replies.put(json.loads(line))
            finally:
                replies.put(None)

        reader = pool.submit(read_reports)
        try:
            if replies.get(timeout=150) != {"ready": True}:
                raise RuntimeError("probe failed to initialize")
            for cycle in range(10):
                # The native probe supplies its own asynchronous filmstrip. Both
                # other background classes do real work during each play/scrub.
                jobs = [pool.submit(run_job, kind, root, work, cycle) for kind in ("waveform", "proxy")]
                child.stdin.write("cycle\n")
                child.stdin.flush()
                row = replies.get(timeout=90)
                if not row or row["cycle"] != cycle + 1:
                    raise RuntimeError(f"probe exited during cycle {cycle + 1}")
                row["jobs"] = {kind: job.result() for kind, job in zip(("waveform", "proxy"), jobs)}
                # Both native mailboxes have been consumed and both FFmpeg jobs have exited.
                row["drained_rss_kib"] = rss_kib()
                row["elapsed_s"] = round(time.monotonic() - origin, 3)
                cycles.append(row)
            child.stdin.close()
            if child.wait(timeout=20) != 0:
                raise RuntimeError("probe did not shut down cleanly")
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            stop.set()
            reader.result()
            watch.result()

    peak = max(r["rss_kib"] for r in samples)
    growth = cycles[-1]["drained_rss_kib"] - cycles[0]["drained_rss_kib"]
    checks = {"peak_under_1_5_gib": peak <= 1536 * 1024,
              "tenth_minus_first_warm_within_64_mib": growth <= 64 * 1024}
    report = {"scope": "Headless preview, filmstrip and two background FFmpeg jobs",
              "fixture": json.loads((root / "fixture-receipt.json").read_text())["sha256"],
              "cpu_rss_kib_peak": peak, "drained_rss_kib_growth": growth,
              "checks": checks, "cycles": cycles, "rss_trace": samples}
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"checks": checks, "peak_mib": peak / 1024, "growth_mib": growth / 1024}))
    if not all(checks.values()):
        raise SystemExit(1)


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("project", type=Path)
    cli.add_argument("--probe", type=Path, required=True)
    cli.add_argument("--out", type=Path, required=True)
    measure(cli.parse_args())
