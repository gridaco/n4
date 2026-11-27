"""Run the native device probe while filmstrip, proxy and waveform workers compete."""
import argparse
import concurrent.futures
import json
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time


def run_job(kind, root, work, iteration):
    pos = str((iteration * 17 + {"filmstrip": 5, "proxy": 29, "waveform": 51}[kind]) % 110)
    cmd = ["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-y", "-filter_threads", "1",
           "-threads", "2", "-ss", pos]
    if kind == "waveform":
        output = work / "waveform.png"
        cmd.extend(["-t", "6", "-i", root / "voice-cue.wav", "-filter_complex_threads", "1",
                    "-filter_complex", "showwavespic=s=512x64:colors=white", "-frames:v", "1"])
    else:
        cmd.extend(["-t", "2", "-i", root / "long-gop.mp4", "-an"])
        if kind == "filmstrip":
            output = work / "filmstrip.png"
            cmd.extend(["-vf", "fps=2,scale=160:90,tile=4x1", "-frames:v", "1"])
        else:
            output = work / "proxy.mp4"
            cmd.extend(["-vf", "scale=640:360", "-c:v", "libx264", "-preset", "veryfast", "-crf", "28"])
    subprocess.run([*cmd, "-threads", "1", output], check=True, timeout=60)
    size = output.stat().st_size
    if not size:
        raise RuntimeError(f"{kind} produced an empty result")
    return {"source_second": int(pos), "bytes": size}


def worker(kind, root, work, stop, origin, events):
    index = 0
    while not stop.is_set():
        begin = time.monotonic()
        result = run_job(kind, root, work, index)
        end = time.monotonic()
        events.append({"kind": kind, "start_s": round(begin - origin, 3),
                       "end_s": round(end - origin, 3), **result})
        index += 1
        stop.wait(max(0., 2. - (end - begin)))


def rss_kib():
    rows = [line.split() for line in subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,rss="], text=True).splitlines()]
    processes = {int(pid): (int(ppid), int(rss)) for pid, ppid, rss in rows}
    group = {os.getpid()}
    while True:
        children = {pid for pid, (ppid, _) in processes.items() if ppid in group}
        if children <= group:
            return sum(processes[pid][1] for pid in group if pid in processes)
        group |= children


def run(args):
    project = args.project.resolve()
    root = project.parent
    out = args.out.resolve()
    device = args.device.resolve()
    with device.open("rb") as binary:
        digest = hashlib.sha256()
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(chunk)
        binary_sha256 = digest.hexdigest()
    out.mkdir()
    cmd = [str(device), str(project), "--seconds", str(args.seconds), "--seek-count",
           str(args.seek_count), "--report", str(out / "device.json")]
    start = time.monotonic()
    stop = threading.Event()
    events, memory = [], []
    ok = False
    with tempfile.TemporaryDirectory(prefix="n4-load-") as temporary:
        work = Path(temporary)
        kinds = [] if args.unloaded else ["filmstrip", "waveform", "proxy"]
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            jobs = [pool.submit(worker, kind, root, work, stop, start, events) for kind in kinds]
            try:
                # Start playback only once all requested job types have produced real outputs.
                while any(not any(e["kind"] == kind for e in events) for kind in kinds):
                    for future in jobs:
                        if future.done():
                            future.result()
                    if time.monotonic() - start > 70:
                        raise TimeoutError("background workers did not become ready")
                    time.sleep(0.05)
                with (out / "stdout.txt").open("w") as stdout, (out / "stderr.txt").open("w") as stderr:
                    probe_start = time.monotonic()
                    child = subprocess.Popen(cmd, stdout=stdout, stderr=stderr)
                    try:
                        while child.poll() is None:
                            for future in jobs:
                                if future.done():
                                    future.result()
                            if time.monotonic() - probe_start > args.seconds + 300:
                                raise TimeoutError("device did not finish the measured run")
                            memory.append({"s": round(time.monotonic() - start, 3),
                                           "process_tree_rss_kib": rss_kib()})
                            time.sleep(1.)
                        if child.returncode:
                            raise subprocess.CalledProcessError(child.returncode, cmd)
                    finally:
                        if child.poll() is None:
                            child.kill()
                            child.wait()
                device_seconds = time.monotonic() - probe_start
                if device_seconds + 0.25 < args.seconds:
                    raise RuntimeError("device exited before the requested playback duration")
                json.loads((out / "device.json").read_text())
                for kind in kinds:
                    group = [e for e in events if e["kind"] == kind]
                    if len(group) < 2 or time.monotonic() - start - group[-1]["end_s"] > 8.:
                        raise RuntimeError(f"{kind} failed to sustain background work")
                ok = True
            finally:
                stop.set()
                try:
                    for future in jobs:
                        future.result()
                except BaseException:
                    ok = False
                    raise
                finally:
                    summary = {"finished": ok, "binary_sha256":binary_sha256,
                               "fixture":json.loads((root / "fixture-receipt.json").read_text()),
                               "device_command": cmd, "background_interval_s": 2,
                               "elapsed_s": round(time.monotonic() - start, 3),
                               "peak_process_tree_rss_kib": max((r["process_tree_rss_kib"] for r in memory), default=None),
                               "events": sorted(events, key=lambda e: e["start_s"]), "rss": memory}
                    (out / "load.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(out)


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("project", type=Path)
    cli.add_argument("--device", required=True, type=Path)
    cli.add_argument("--out", required=True, type=Path)
    cli.add_argument("--seconds", default=120, type=int)
    cli.add_argument("--seek-count", default=30, type=int)
    cli.add_argument("--unloaded", action="store_true")
    args = cli.parse_args()
    if not 1 <= args.seconds <= 120 or args.seek_count < 0:
        cli.error("seconds must be in [1,120] and seek-count nonnegative")
    run(args)
