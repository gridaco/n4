"""Render and check both editable creator projects. Requires a Metal device."""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
FIXTURES = ("product", "product-revised", "chart", "chart-revised")


def run(cmd):
    return subprocess.run([str(x) for x in cmd], check=True, capture_output=True, text=True).stdout


def render(cli, edit, seek, dest):
    run([cli, "render", edit, "--frames", ",".join(map(str, seek)), "--out", dest])
    return json.loads((dest / "receipt.json").read_text())["frames"]


def check(cli, out, movies):
    out.mkdir(parents=True, exist_ok=True)
    results = {}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for key in FIXTURES:
            edit = HERE / (key + ".json")
            seek = [60, 110, 60] if key.startswith("product") else [90, 185, 90]
            pngs = work / key
            frames = render(cli, edit, seek, pngs)
            images = [Image.open(pngs / row["file"]).convert("RGB") for row in frames]
            assert np.array_equal(images[0], images[2]), "backward seek changed the same frame"
            images[0].save(out / (key + ".png"))
            results[key] = {"frame": seek[0], "title_lines": frames[0]["trace"]["paragraph_lines"],
                            "first_paint": frames[0]["trace"]["graphics"],
                            "repeat_paint": frames[-1]["trace"]["graphics"]}
            assert results[key]["repeat_paint"]["graphic_uploads"] == 0
            if movies:
                movie = out / (key + ".mp4")
                export = json.loads(run([cli, "export", edit, "--out", movie]))
                (out / (key + "-export.json")).write_text(json.dumps(export, indent=2) + "\n")
                streams = json.loads(run(["ffprobe", "-v", "error", "-show_entries",
                                          "stream=codec_name,nb_frames,sample_rate:format=duration", "-of", "json", movie]))
                video, audio = streams["streams"]
                assert video["codec_name"] == "h264" and audio["codec_name"] == "aac"
                assert int(video["nb_frames"]) == (225 if key.startswith("chart") else 180)
                results[key]["duration_seconds"] = streams["format"]["duration"]
        assert results["product"]["title_lines"] < results["product-revised"]["title_lines"]
        a = np.asarray(Image.open(out / "product.png"))
        b = np.asarray(Image.open(out / "product-revised.png"))
        # At frame 60 take A has toggled; take B still has hum. Probe the switch away from the cursor.
        assert a[415, 813, 1] > a[415, 813, 0] + 10
        assert np.linalg.norm(a[415, 813].astype(float) - b[415, 813]) > 40
        base = json.loads((HERE / "chart.json").read_text())
        # Resolve paths so independent one-variable variants can live in a temporary directory.
        base["title"]["font"] = str(HERE / base["title"]["font"])
        for item in base["shots"] + base["audio"]:
            item["source"] = str(HERE / item["source"])
        for layer in base["graphics"]:
            for field in ("font", "source"):
                if field in layer["content"]:
                    layer["content"][field] = str(HERE / layer["content"][field])
            if "filter" in layer:
                layer["filter"]["shader"] = str(HERE / layer["filter"]["shader"])
        original = np.array(Image.open(out / "chart.png"))
        for kind in ("ink", "timing"):
            edit = json.loads(json.dumps(base))
            if kind == "ink":
                edit["graphics"][0]["filter"]["amount"] = 82
            else:
                edit["graphics"][1]["wipe"] = [24, 177]
            path = work / (kind + ".json")
            path.write_text(json.dumps(edit))
            pngs = work / kind
            frame = render(cli, path, [90], pngs)[0]
            changed = np.any(np.array(Image.open(pngs / frame["file"]).convert("RGB")) != original, axis=2)
            y, x = np.where(changed)
            assert len(x) > 15
            if kind == "ink":
                assert np.all((109 <= x) & (x < 895) & (172 <= y) & (y < 388))
                assert np.all((np.min(np.abs(y[:, None] - [175, 242, 308, 375]), axis=1) <= 2) | (y >= 371))
            else:
                assert np.all((x >= 109 + 786 * (90 - 24) // 153) & (x < 109 + 786 * (90 - 18) // 153))
            results["chart-" + kind + "-only"] = {"changed_pixels": int(len(x)), "bounds_inclusive": list(map(int, [x.min(), y.min(), x.max(), y.max()]))}
    (out / "checks.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("cli", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--movies", action="store_true")
    opts = parser.parse_args()
    check(opts.cli.resolve(), opts.out.resolve(), opts.movies)
