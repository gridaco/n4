"""Verify the animated frosted caption through the public render, edit and export CLI."""
import argparse
from copy import deepcopy
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "av-cut"))
from compare_preview import check_preview
from session_replay import session


def pictures(cli, project, out, frames):
    subprocess.run([cli, "render", project, "--frames", ",".join(map(str, frames)),
                    "--out", out], check=True, stdout=subprocess.DEVNULL)
    result = []
    for i, frame in enumerate(frames):
        with Image.open(out / f"{i:03}-f{frame:04}.png") as image:
            assert image.mode == "RGBA"
            result.append(np.array(image))
    return result


def difference(a, b):
    return np.any(a != b, axis=2)


def run(cli, out):
    frame = 96
    out.mkdir(parents=True, exist_ok=True)
    products = ["demo.mp4", "frame-0096.png", "comparison.png", "checks.json"]
    if any((out / name).exists() for name in products):
        raise ValueError("choose a fresh output directory")
    source = json.loads((HERE / "frost.json").read_text())
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        work = root / "creator"
        work.mkdir()
        (root / "fonts").symlink_to(HERE.parent / "fonts", target_is_directory=True)
        for name in ["take-b.mp4", "take-b.wav", "gaussian-blur.wgsl", "frost-mask.wgsl"]:
            (work / name).symlink_to(HERE / name)

        def cut(name, edit):
            file = work / (name + ".json")
            file.write_text(json.dumps(edit))
            return file

        def control(name, edit):
            return pictures(cli, cut(name, edit), root / name, [frame])[0]

        canonical = cut("frost", source)
        original, early, returned = pictures(cli, canonical, root / "baseline", [frame, 22, frame])
        assert np.array_equal(original, returned)
        assert difference(original[228:355, 820:885], early[228:355, 820:885]).sum() > 100
        height, width = original.shape[:2]
        x, y = source["graphics"][2]["position"]
        card_w, card_h = 330, 96
        yy, xx = np.mgrid[:height, :width]
        inside = (xx >= x) & (xx < x + card_w) & (yy >= y) & (yy < y + card_h)
        interior = ((xx >= x + 48) & (xx < x + card_w - 48)
                    & (yy >= y + 26) & (yy < y + card_h - 26))

        zero = deepcopy(source)
        zero["graphics"][2]["content"]["filter"]["sigma_px"] = 0
        sharp = control("zero", zero)
        missing = deepcopy(source)
        missing["graphics"].pop(2)
        assert np.array_equal(sharp, control("without-card", missing))
        changed = difference(original, sharp)
        assert changed.sum() > 6_000 and not changed[~inside].any()

        raised = deepcopy(source)
        layers = raised["graphics"]
        layers[1], layers[2] = layers[2], layers[1]
        above = control("stripe-above", raised)
        stroke_color = source["graphics"][1]["content"]["stroke"]["rgba"]
        solid_trace = np.all(sharp == stroke_color, axis=2)
        crossing = solid_trace & interior
        assert crossing.sum() > 25
        raised_delta = np.abs(above[crossing].astype(np.int16) - sharp[crossing]).max(axis=1)
        assert raised_delta.max() <= 1, (crossing.sum(), np.unique(raised_delta, return_counts=True))
        assert difference(above, original)[crossing].sum() > 25
        glyph_color = source["graphics"][3]["content"]["rgb"]
        solid_glyph = np.all(sharp[:, :, :3] == glyph_color, axis=2) & interior
        glyph_delta = np.abs(sharp[solid_glyph].astype(np.int16) - original[solid_glyph]).max(axis=1)
        assert solid_glyph.sum() > 15 and glyph_delta.max() <= 1, (solid_glyph.sum(), np.unique(glyph_delta, return_counts=True))

        # Sample exactly the same underlying video and traced curve, changing
        # only the mask phase. All differences must be along the card boundary.
        phase_edit = deepcopy(source)
        phase_edit["graphics"][2]["mask"]["shader"]["period_frames"] = 160
        alternate = control("mask-phase", phase_edit)
        phase_delta = difference(original, alternate)
        assert phase_delta.sum() > 150 and not phase_delta[interior | ~inside].any()

        sigma_pointer = "/graphics/2/content/filter/sigma_px"
        with session(cli, canonical) as edit:
            edit("inspect", frame=frame)
            assert edit("set", at=0, path=sigma_pointer, value=-2, ok=False)["revision"] == 0
            edit("set", at=0, path=sigma_pointer, value=2)
            changed_sigma = edit("inspect", frame=frame)["result"]
            assert changed_sigma["reused_decode"] and changed_sigma["graphics"]["graphic_uploads"] == 0
            edit("save", at=1)
            weaker = pictures(cli, canonical, root / "lower-sigma", [frame])[0]
            assert difference(weaker, original).sum() > 4_000
            edit("set", at=1, path="/graphics/2/position", value=[x + 38, y + 10])
            changed_position = edit("inspect", frame=frame)["result"]
            assert changed_position["reused_decode"] and changed_position["graphics"]["graphic_uploads"] == 0
            edit("save", at=2)
            shifted = pictures(cli, canonical, root / "shifted", [frame])[0]
            assert difference(weaker, shifted).sum() > 500
            edit("undo", at=2)
            edit("undo", at=3)
            edit("save", at=4)
        with session(cli, canonical) as restored_session:
            assert restored_session("inspect", frame=frame)["revision"] == 0
        restored = pictures(cli, canonical, root / "reopened", [frame])[0]
        assert np.array_equal(restored, original)

        movie = out / "demo.mp4"
        export = json.loads(subprocess.check_output([cli, "export", canonical, "--out", movie]))
        report = root / "encoder.json"
        report.write_text(json.dumps(export))
        parity = check_preview(cli, canonical, report)
        streams = json.loads(subprocess.check_output([
            "ffprobe", "-v", "error", "-count_frames", "-show_entries",
            "stream=codec_type,codec_name,width,height,avg_frame_rate,sample_rate,nb_read_frames",
            "-of", "json", movie,
        ]))["streams"]
        video = next(s for s in streams if s["codec_type"] == "video")
        audio = next(s for s in streams if s["codec_type"] == "audio")
        assert int(video["nb_read_frames"]) == parity["inspection_frames"] == 180
        assert (video["codec_name"], video["width"], video["height"], video["avg_frame_rate"]) == (
            "h264", 960, 540, "30/1",
        )
        assert (audio["codec_name"], audio["sample_rate"]) == ("aac", "48000")
        Image.fromarray(original).save(out / "frame-0096.png")
        # Show zero, blur, and reordering with identical video, text and source time.
        crops = [picture[y-14:y+card_h+14, x-18:x+card_w+18] for picture in (sharp, original, above)]
        Image.fromarray(np.vstack(crops)).save(out / "comparison.png")
        receipt = dict(
            parity, decoded_delivery=streams, frame=frame, size=source["size"],
            control_point="same source frame and sharp label for every ordering comparison",
            blur_changed_pixels=int(changed.sum()), changed_outside_card=int(changed[~inside].sum()),
            mask_phase_changed_pixels=int(phase_delta.sum()), stripe_crossing_pixels=int(crossing.sum()),
            sharp_caption_interior_pixels=int(solid_glyph.sum()),
            sigma_edit_graphic_uploads=changed_sigma["graphics"]["graphic_uploads"],
            moved_card_graphic_uploads=changed_position["graphics"]["graphic_uploads"],
            native_video_decode_reused=True, reverse_seek=[frame, 22, frame],
            zero_matches_removed=True, undo_save_reopen_matches=True,
        )
        (out / "checks.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("out", type=Path)
    args = parser.parse_args()
    run(args.cli.resolve(), args.out.resolve())
