"""Verify reflow bounds and artwork reuse on a parked native picture."""
import argparse
import hashlib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent


def check(cli, out):
    out.mkdir(parents=True, exist_ok=True)
    project = HERE / "material.json"
    svg = HERE / "stereo-field.svg"
    before = hashlib.sha256(svg.read_bytes()).hexdigest()
    commands = [{"op": "inspect", "frame":5}, {"op":"set","at":0,"path":"/title/width","value":220},
                {"op":"inspect","frame":5}, {"op":"inspect","frame":5}]
    response = subprocess.run([cli,"session",project], check=True, text=True, capture_output=True,
                              input="".join(json.dumps(c)+"\n" for c in commands))
    responses = [json.loads(line) for line in response.stdout.splitlines()]
    assert all(row["ok"] for row in responses)
    wide, narrow, parked = [responses[i]["result"] for i in (0,2,3)]
    a, b = wide["backplate_rect"], narrow["backplate_rect"]
    assert a[:2] == b[:2] == [54,85] and a[2] > b[2] and a[3] < b[3]
    assert narrow["reused_decode"] and narrow["graphics"]["graphic_uploads"] == 0
    assert parked["reused_paragraph"] and not narrow["reused_paragraph"]
    with tempfile.TemporaryDirectory() as temp:
        for name, width in [("wide","600"), ("narrow","220")]:
            dest = Path(temp) / name
            subprocess.run([cli,"render",project,"--frames","5","--title-width",width,"--out",dest],check=True)
            shutil.copyfile(dest/"000-f0005.png", out/("material-"+name+".png"))
        large = np.asarray(Image.open(out/"material-wide.png"))
        small = np.asarray(Image.open(out/"material-narrow.png"))
        diff = np.any(large != small, axis=2)
        changed = int(diff.sum())
        for x,y,w,h in [a,b]: diff[y:y+h,x:x+w] = False
        assert changed > 1000 and not np.any(diff)
    assert hashlib.sha256(svg.read_bytes()).hexdigest() == before
    evidence = {"frame":5, "wide": {"plate":a,"lines":wide["paragraph_lines"]},
                "narrow":{"plate":b,"lines":narrow["paragraph_lines"]}, "changed_pixels":changed,
                "changed_outside_backplates":0, "reused_decode_on_width_edit":narrow["reused_decode"],
                "static_graphic_uploads_on_width_edit":narrow["graphics"]["graphic_uploads"],
                "reused_paragraph_on_repeat":parked["reused_paragraph"], "untouched_svg_sha256":before}
    (out/"material-checks.json").write_text(json.dumps(evidence,indent=2)+"\n")


if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("cli",type=Path)
    parser.add_argument("out",type=Path)
    options=parser.parse_args()
    check(options.cli.resolve(),options.out.resolve())
