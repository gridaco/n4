"""Qualify the path-traced Final on the Field Unit reflection shot, through the n4 command line.

Builds nothing. Expects a release n4 built with --features cycles and a provisioned bundle:

    spikes/spatial/cycles/tools/provision.sh
    (cd spikes && cargo build --release -p n4-engine --features cycles --bin n4)
    python3 spikes/spatial/cycles/tools/qualify.py --out .local/cycles/qualify

Each check edits a private copy of the project. The receipt records what was traced, what was
reused and how long it took on this machine.
"""
import argparse, hashlib, json, platform, re, shutil, subprocess, sys, time
from pathlib import Path
import numpy as np
from PIL import Image

REPO = Path(__file__).resolve().parents[4]
N4 = REPO / "spikes/engine/target/release/n4"
MAKE = REPO / "spikes/spatial/cycles/shots/field-unit-reflection/make_project.py"
FRAMES = [4, 25, 50]


def run(*args, env=None):
    out = subprocess.run([str(a) for a in args], capture_output=True, text=True, env=env)
    if out.returncode:
        sys.exit(f"{' '.join(map(str, args))}\n{out.stdout}\n{out.stderr}")
    return out.stdout


def render(project, out, store, frames=FRAMES, final=True):
    """With `final`, trace what is missing. Without it, show stored Finals and Draft the rest."""
    if out.exists():
        shutil.rmtree(out)
    args = [N4, "render", project, "--frames", ",".join(map(str, frames)), "--out", out, "--final-store", store]
    if final:
        args += ["--final"]
    started = time.time()
    run(*args)
    receipt = json.loads((out / "receipt.json").read_text())
    pictures = [np.array(Image.open(out / f["file"]).convert("RGB")).astype(int) for f in receipt["frames"]]
    return {
        "traced": sum(f.get("path_traced_scenes", 0) for f in receipt["frames"]),
        "final": [f["trace"]["graphics"].get("path_traced_final", 0) for f in receipt["frames"]],
        "draft": [f["trace"]["graphics"].get("path_traced_draft", 0) for f in receipt["frames"]],
        "seconds": [round(f.get("seconds", 0), 3) for f in receipt["frames"]],
        "wall": round(time.time() - started, 2),
    }, pictures


def edit(project, name, old, new):
    path = project / "sequences" / name
    text = path.read_text()
    assert old in text, old
    path.write_text(text.replace(old, new, 1))


def changed(a, b, threshold=8):
    return [int((np.abs(x - y).max(axis=2) > threshold).sum()) for x, y in zip(a, b)]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--out", type=Path, required=True)
    args = p.parse_args()
    out = args.out.resolve()
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    project, store = out / "project", out / "store"
    run(sys.executable, MAKE, project)
    r = {"machine": {"model": run("sysctl", "-n", "machdep.cpu.brand_string").strip(), "macos": platform.mac_ver()[0]},
         "n4_sha256": hashlib.sha256(N4.read_bytes()).hexdigest(), "size": [1920, 1080], "recipe": "64 samples, 8 bounces, denoised",
         "frames": FRAMES, "checks": {}}
    c = r["checks"]

    draft, draft_px = render(project, out / "draft", store, final=False)
    c["without --final the scene is a Draft"] = {"pass": draft["draft"] == [1, 1, 1] and draft["final"] == [0, 0, 0], **draft}

    first, base = render(project, out / "final", store)
    c["--final traces each frame once"] = {"pass": first["traced"] == 3 and first["final"] == [1, 1, 1], **first}
    c["draft and final differ"] = {"pass": min(changed(draft_px, base)) > 100_000, "changed_pixels": changed(draft_px, base)}

    again, same = render(project, out / "again", store, frames=FRAMES[::-1])
    identical = all((a == b).all() for a, b in zip(same[::-1], base))
    c["a second process reuses stored pictures, in reverse order"] = {"pass": again["traced"] == 0 and identical, **again, "identical": identical}
    plain, plain_px = render(project, out / "plain", store, final=False)
    c["an ordinary render shows a stored Final"] = {"pass": plain["final"] == [1, 1, 1] and all((a == b).all() for a, b in zip(plain_px, base)), **plain}

    cold, cold_px = render(project, out / "cold", out / "store-cold", frames=FRAMES[::-1])
    worst = max(int(np.abs(a - b).max()) for a, b in zip(cold_px[::-1], base))
    within = min(float((np.abs(a - b).max(axis=2) <= 2).mean()) for a, b in zip(cold_px[::-1], base))
    c["a cold re-render in reverse order is visually the same"] = {"pass": cold["traced"] == 3 and worst <= 6, "largest_difference_of_255": worst, "pixels_within_2": round(within, 6), **cold}

    edit(project, "caption.xml", "PATH TRACED FINAL", "APPROVED 28 NOV")
    cap, cap_px = render(project, out / "caption", store)
    c["a caption edit traces nothing"] = {"pass": cap["traced"] == 0 and min(changed(cap_px, base)) > 500, **cap, "changed_pixels": changed(cap_px, base)}

    edit(project, "reflection.xml", "<copy>FIELD UNIT</copy>", "<copy>FIELD MARK</copy>")
    word, word_px = render(project, out / "word", store)
    delta = [np.abs(a - b).max(axis=2) > 8 for a, b in zip(word_px, cap_px)]
    c["a new word on the screen traces every frame that shows it"] = {
        "pass": word["traced"] == 3 and int(delta[0].sum()) > 2_000, **word,
        "changed_pixels": [int(d.sum()) for d in delta],
        "changed_below_the_lid_on_frame_4": int(delta[0][520:].sum())}

    edit(project, "reflection.xml", 'source-in="0"', 'source-in="3"')
    slip, slip_px = render(project, out / "slip", store)
    c["slipping the footage three frames traces every frame"] = {"pass": slip["traced"] == 3 and min(changed(slip_px, word_px)) > 1_000, **slip, "changed_pixels": changed(slip_px, word_px)}

    edit(project, "reflection.xml", 'center="-0.36 0.52 0.34"', 'center="-0.10 0.52 0.34"')
    light, light_px = render(project, out / "light", store)
    c["moving the key light traces every frame"] = {"pass": light["traced"] == 3 and min(changed(light_px, slip_px)) > 100_000, **light, "changed_pixels": changed(light_px, slip_px)}

    # The whole range, then delivery from the store.
    project2, store2 = out / "film", out / "store-film"
    run(sys.executable, MAKE, project2)
    env = {**__import__("os").environ, "N4_FINAL_STORE": str(store2)}
    started = time.time()
    lines = [json.loads(l) for l in run(N4, "finish", project2, env=env).splitlines() if l.startswith("{")]
    finish = {"seconds": round(time.time() - started, 1), "traced": lines[-1]["traced"],
              "median_frame_seconds": round(float(np.median([l["seconds"] for l in lines[1:-1]])), 3),
              "first_frame_seconds": round(lines[0]["seconds"], 3)}
    started = time.time()
    report = json.loads(re.search(r"\{.*\}", run(N4, "export", project2, "--out", out / "film.mp4", env=env), re.S).group(0))
    export_s = round(time.time() - started, 1)
    retrace = [json.loads(l) for l in run(N4, "finish", project2, env=env).splitlines() if l.startswith("{")][-1]
    again = json.loads(re.search(r"\{.*\}", run(N4, "export", project2, "--out", out / "film-again.mp4", env=env), re.S).group(0))
    c["the range is traced once, then exported from the store"] = {
        "pass": finish["traced"] == 51 and retrace["traced"] == 0 and retrace["reused"] == 51 and again["rgba_sha256"] == report["rgba_sha256"],
        "finish": finish, "export_seconds": export_s, "frames": report["frames"], "encoder_input_sha256": report["rgba_sha256"],
        "second_export_same_input": again["rgba_sha256"] == report["rgba_sha256"],
        "store_megabytes": round(sum(f.stat().st_size for f in store2.iterdir()) / 2**20, 1)}

    r["passed"] = all(v["pass"] for v in c.values())
    (out / "receipt.json").write_text(json.dumps(r, indent=1) + "\n")
    for name, v in c.items():
        print("PASS" if v["pass"] else "FAIL", name)
    print("receipt", out / "receipt.json")
    sys.exit(0 if r["passed"] else 1)


if __name__ == "__main__":
    main()
