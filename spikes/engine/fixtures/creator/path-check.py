"""Verify the native Path cut, edit replay and full-quality inspection/export parity."""
import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "av-cut"))
from compare_preview import check_preview
from session_replay import session


def picture(cli, project, root, name, frames):
    destination = root / name
    subprocess.run(
        [cli, "render", project, "--frames", ",".join(map(str, frames)), "--out", destination],
        check=True, stdout=subprocess.DEVNULL,
    )
    result = []
    for index, frame in enumerate(frames):
        with Image.open(destination / f"{index:03}-f{frame:04}.png") as image:
            result.append(np.array(image))
    return result


def changed_pixels(a, b):
    return np.any(a != b, axis=2)


def check(cli, out):
    # Keep shipped source assets read-only. The session saves solely to its temporary copy.
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ValueError(f"evidence directory must be empty: {out}")
    source = HERE / "path.json"
    document = json.loads(source.read_text())
    frame = 96
    mask_pointer = "/graphics/0/content/children/0/mask/content/d"
    group = document["graphics"][0]
    x, y = group["position"]
    contour = group["content"]["children"][0]["mask"]["content"]["d"]
    assert contour.count("92 6 110 6") == 1
    edited_curve = contour.replace("92 6 110 6", "92 80 110 6")
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        work = root / "creator"
        work.mkdir()
        (root / "fonts").symlink_to(HERE.parent / "fonts", target_is_directory=True)
        for name in ("take-b.mp4", "take-b.wav", "dry-ink.wgsl"):
            (work / name).symlink_to(HERE / name)
        project = work / "path.json"
        shutil.copyfile(source, project)
        original, earlier, back = picture(cli, project, root, "original", [frame, 22, frame])
        assert np.array_equal(original, back)

        # Remove only the native waveform; title, labels and underlying video remain identical.
        reference = json.loads(source.read_text())
        reference["graphics"].pop(0)
        base = work / "base.json"
        base.write_text(json.dumps(reference))
        plate, earlier_plate = picture(cli, base, root, "without-waveform", [frame, 22])
        composed = changed_pixels(original, plate)
        assert composed.sum() > 4_000
        assert composed[y + 18, x + 110] and not composed[y + 5, x + 145]
        assert np.array_equal(original[:, 380:], plate[:, 380:])
        assert np.any(plate[:, 380:] != earlier_plate[:, 380:]), "source movie did not advance"
        assert np.any(original[y:y + 130, x:x + 277] != earlier[y:y + 130, x:x + 277])

        with session(cli, project) as send:
            send("inspect", frame=frame)
            rejected = send("set", ok=False, at=0, path=mask_pointer, value="M ?")
            assert rejected["revision"] == 0
            send("set", at=0, path=mask_pointer, value=edited_curve)
            edited_trace = send("inspect", frame=frame)["result"]
            assert edited_trace["reused_decode"]
            send("save", at=1)
            edited = picture(cli, project, root, "control-point", [frame])[0]
            changed = changed_pixels(edited, original)
            changed_count = int(changed.sum())
            assert changed_count > 60, "control-point edit failed to change mask coverage"
            ys, xs = np.where(changed)
            assert xs.min() >= x - 6 and xs.max() <= x + 280
            assert ys.min() >= y + 4 and ys.max() <= y + 83
            channel_delta = np.abs(edited.astype(np.int16) - original.astype(np.int16)).max(axis=2)
            # The altered cubic and all four control points lie within this local box.
            outside_curve = channel_delta.copy()
            outside_curve[y + 6:y + 82, x + 73:x + 111] = 0
            # The curve edit should uncover the plate under the former hump.
            assert np.array_equal(edited[y + 42, x + 98], plate[y + 42, x + 98])
            assert not np.array_equal(original[y + 42, x + 98], plate[y + 42, x + 98])

            dx, dy = 18, 8
            send("set", at=1, path="/graphics/0/position", value=[x + dx, y + dy])
            moved_trace = send("inspect", frame=frame)["result"]
            assert moved_trace["graphics"]["graphic_uploads"] == 0
            send("inspect", frame=22)
            send("inspect", frame=frame)
            send("save", at=2)
            moved = picture(cli, project, root, "translated", [frame])[0]
            # Opaque interior should retain its color under one parent translation.
            for px, py in [(110, 30), (73, 101), (149, 115), (240, 75)]:
                assert np.array_equal(edited[y + py, x + px], moved[y + dy + py, x + dx + px])
            assert np.count_nonzero(changed_pixels(edited, moved)) > 4_000

            send("undo", at=2)
            send("undo", at=3)
            send("save", at=4)
        with session(cli, project) as reopened:
            state = reopened("inspect", frame=frame)
            assert state["revision"] == 0
        restored = picture(cli, project, root, "reopened", [frame])[0]
        assert np.array_equal(restored, original)

        movie = out / "demo.mp4"
        export = json.loads(subprocess.check_output([cli, "export", project, "--out", movie]))
        report = root / "encoder.json"
        report.write_text(json.dumps(export))
        parity = check_preview(cli, project, report)
        streams = json.loads(subprocess.check_output([
            "ffprobe", "-v", "error", "-count_frames",
            "-show_entries", "stream=codec_type,codec_name,width,height,avg_frame_rate,sample_rate,nb_read_frames",
            "-of", "json", movie,
        ]))["streams"]
        video = next(row for row in streams if row["codec_type"] == "video")
        assert int(video["nb_read_frames"]) == parity["inspection_frames"] == 180
        audio = next(row for row in streams if row["codec_type"] == "audio")
        assert (video["codec_name"], video["width"], video["height"], video["avg_frame_rate"]) == (
            "h264", 960, 540, "30/1",
        )
        assert (audio["codec_name"], audio["sample_rate"]) == ("aac", "48000")
        Image.fromarray(original).save(out / "frame-0096.png")
        receipt = dict(
            parity, frame=frame, size=document["size"], fps=document["fps"],
            decoded_delivery=streams, source="take-b.mp4", waveform_pixels=int(composed.sum()),
            control_point_edit_pixels=changed_count,
            control_point_change_bounds_xyxy_inclusive=[int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())],
            changed_pixels_outside_curve_box=int(np.count_nonzero(outside_curve)),
            peak_channel_delta_outside_curve_box=int(outside_curve.max()),
            group_translation=[dx, dy], translation_uploads=moved_trace["graphics"]["graphic_uploads"],
            invalid_edit_kept_revision=rejected["revision"], reversed_frame_order=[frame, 22, frame],
            undo_reopen_pixel_identical=True,
        )
        (out / "checks.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("out", type=Path, help="new evidence directory")
    args = parser.parse_args()
    check(args.cli.resolve(), args.out.resolve())
