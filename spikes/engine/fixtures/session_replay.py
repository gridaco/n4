"""Exercise the public session command with an independent text edit and a broken linked render."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
from contextlib import contextmanager


@contextmanager
def session(binary, project):
    with subprocess.Popen([binary, "session", project], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          text=True, encoding="utf-8") as child:
        def send(op, ok=True, **fields):
            child.stdin.write(json.dumps({"op": op, **fields}) + "\n")
            child.stdin.flush()
            reply = json.loads(child.stdout.readline())
            if reply["ok"] != ok:
                raise AssertionError(reply)
            return reply
        try:
            yield send
        finally:
            child.stdin.close()


def replay(binary):
    fixtures = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory() as folder:
        root = Path(folder)
        work = root / "creator"
        work.mkdir()
        (root / "fonts").symlink_to(fixtures / "fonts", target_is_directory=True)
        for asset in (fixtures / "creator").iterdir():
            if asset.suffix in {".svg", ".mp4", ".wgsl", ".wav"}:
                (work / asset.name).symlink_to(asset)
        project = work / "chart.json"
        shutil.copyfile(fixtures / "creator/chart.json", project)
        with session(binary, project) as send:
            send("inspect", frame=90)
            send("set", at=0, path="/graphics/0/filter/amount", value=82)
            parked = send("inspect", frame=90)["result"]
            assert parked["reused_decode"] and parked["graphics"]["raster_paints"] == 0
            send("save", at=1)
            raw = project.read_text()
            # Locate the authored property exactly as a terminal editor would.
            hit = subprocess.check_output(["rg", "-n", '"wipe"', project], text=True)
            external = json.loads(raw)
            external["graphics"][1]["wipe"] = [24, 177]
            project.write_text(json.dumps(external, indent=2) + "\n")
            send("reload", at=1)
            send("undo", at=2)
            send("redo", at=3)
            send("set", ok=False, at=2, path="/graphics/0/filter/amount", value=10)
            send("save", at=4)
        with session(binary, project) as send:
            restored = send("inspect", frame=90)
            assert restored["revision"] == 0
        delivered = root / "accepted.mp4"
        export = json.loads(subprocess.check_output([binary, "export", project, "--out", delivered], text=True))
        oracle = json.loads(subprocess.check_output([binary, "export", fixtures / "creator/chart-revised.json",
            "--out", root / "expected.mp4"], text=True))
        assert export["rgba_sha256"] == oracle["rgba_sha256"]
        stream = subprocess.check_output(["ffprobe", "-v","error", "-select_streams","v:0", "-count_frames",
                                         "-show_entries","stream=nb_read_frames", "-of","csv=p=0", delivered], text=True)
        assert int(stream.strip()) == export["frames"] == 225
        # Use a generated movie twice with different trims and publish a producer's new output.
        linked = work / "linked.json"
        edit = json.loads((fixtures / "creator/product.json").read_text())
        edit["shots"] = [{"source":"take-a.mp4","source_in":offset,"duration":90} for offset in (0,32)]
        linked.write_text(json.dumps(edit, indent=2))
        with session(binary, linked) as send:
            send("relink", at=0, **{"from":"take-a.mp4","to":"take-b.mp4"})
            a = send("inspect", frame=15)["result"]["request"]
            b = send("inspect", frame=105)["result"]["request"]
            (work / "unfinished.mp4").write_bytes(b"incomplete producer output")
            send("relink", at=1, ok=False, **{"from":"take-b.mp4","to":"unfinished.mp4"})
            assert send("inspect", frame=105)["result"]["request"] == b
            send("save", at=1)
        with session(binary, linked) as send:
            assert send("inspect", frame=15)["result"]["request"] == a
        assert [a["source_frame"], b["source_frame"]] == [15,47]
        assert all(Path(x["source"]).name == "take-b.mp4" for x in (a,b))
        return {"delivered_reopened_chart":export, "independently_decoded_frames":225, "parked_graphics": parked["graphics"], "reused_decode": parked["reused_decode"],
                "external_property":hit.strip().split(":",1)[-1].strip(), "undo_redo_stale_and_reopen":True,
                "linked_trim_frames":[15,47],"interrupted_source_retained":"take-b.mp4",
                "saved_chart_sha256":hashlib.sha256(project.read_bytes()).hexdigest()}


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("binary", type=Path)
    cli.add_argument("report", type=Path)
    args = cli.parse_args()
    result = replay(args.binary.resolve())
    with args.report.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
