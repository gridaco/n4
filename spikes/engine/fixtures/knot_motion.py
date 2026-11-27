"""Build an editable ring-to-trefoil motion study. Frame zero is on the opening ring."""
import argparse
from array import array
import cmath
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import wave

from knot_crossing import cubic_window

HERE = Path(__file__).resolve().parent
FRAMES, FPS, SAMPLE_RATE = 180, 30, 48_000
RELEASE = 18
SEGMENTS = 6
TAU = math.tau


def screen(z):
    """In the drawing plane, angle zero faces up and positive angles turn clockwise."""
    return [round(960 + z.imag, 4), round(540 - z.real, 4)]


def cubic_controls(curve, segments):
    """Hermite-fit a smooth design curve with corresponding cubic segments at every pose."""
    knots = [i / segments for i in range(segments + 1)]
    h = 1.e-5
    points = [screen(curve(0))]
    for a, b in zip(knots, knots[1:]):
        deriv_a = (curve(a + h) - curve(a - h)) / (2*h)
        deriv_b = (curve(b + h) - curve(b - h)) / (2*h)
        reach = (b - a) / 3
        points.extend([screen(curve(a) + reach * deriv_a),
                       screen(curve(b) - reach * deriv_b), screen(curve(b))])
    return points


def ring_controls(start, end, radius, segments):
    """Split at fractions of the analytic circle's circumference. End tangents agree exactly."""
    step = -TAU * (end - start) / segments
    handle = 4/3 * math.tan(step / 4)
    first = radius * cmath.exp(-1j * TAU * start)
    points = [screen(first)]
    for i in range(1, segments + 1):
        last = radius * cmath.exp(-1j * TAU * (start + (end - start) * i / segments))
        points.extend([screen(first + 1j * handle * first),
                       screen(last - 1j * handle * last), screen(last)])
        first = last
    return points


def strand_poses(art, index):
    shift = art["seam_shift_turns"]
    pull = art["tip_pull_px"] if index == 0 else 0
    seams = [0, 1/3 + shift, 2/3, 1]
    start, end = seams[index:index + 2]
    radius = art["ring_radius"]

    def geometry(stage, u):
        heading = -TAU * (start + (end - start) * u)
        t = (index + u) * TAU / 3
        ring = radius * cmath.exp(1j * heading)
        # Fourier projection of a trefoil. Each third shares a tangent with its
        # neighbor at the final tip. There are three non-endpoint crossings.
        lock = 222 * cmath.exp(2j*t) + 123 * cmath.exp(-1j*t) - 45
        tip = u**4
        belly = math.sin(math.pi * u)**2
        peeled = (radius + 9*belly + (82 + .55*pull)*tip) * cmath.exp(1j*(heading + .31*tip))
        fanned = (radius + 105*belly + (112 + pull)*u**3) * cmath.exp(
            1j*(heading + .50*u**2 + .16*belly - .08))
        if stage == 1:
            return peeled
        if stage == 2:
            return fanned
        if stage == 3:
            # The end leads the belly through the turn. One tip can pull farther
            # without asking either neighboring strand for a different pose.
            progress = .54 + .32*u*u - .12*belly
            return fanned*(1-progress) + lock*progress + cmath.exp(1j*heading)*(28 + .22*pull)*belly
        if stage == 4:
            # A slight overtravel keeps tension in the turn before the latch.
            return lock + .074*(lock-ring) + 8*cmath.exp(1j*heading)*belly
        return lock

    frames = list(art["poses"][index])
    if index == 0:
        frames[3] += art["weave_delay_frames"]
        frames[4] += art["weave_delay_frames"]
    if len(frames) != 6 or list(sorted(set(frames))) != frames or frames[0] < RELEASE or frames[-1] >= FRAMES:
        raise ValueError("each strand needs six increasing pose frames between release and the final hold")
    keys = []
    for stage, frame in enumerate(frames):
        points = (ring_controls(start, end, radius, SEGMENTS) if stage == 0 else
                  cubic_controls(lambda u: geometry(stage, u), SEGMENTS))
        keys.append({"frame":frame - RELEASE, "ease":"spline", "points":points})
    return {"keys":keys}


def stroke(d, width, rgba):
    return {"content":{"type":"path", "d":d, "fill":None,
                       "stroke":{"width":width, "rgba":rgba, "cap":"round", "join":"round"}}}


def layout(art):
    width = art["stroke_width"]
    source_ring = ring_controls(0, 1, art["ring_radius"], 24)
    # The unbroken hold has a single edge at every seam. Starting the first open
    # curves at the same coordinates also leaves their tips independently editable.
    pair = lambda p: " ".join(str(v) for v in p)
    commands = ["M " + pair(source_ring[0])]
    for i in range(1, len(source_ring), 3):
        commands.append("C " + " ".join(pair(p) for p in source_ring[i:i+3]))
    graphics = [{**stroke(" ".join(commands) + " Z", width, art["ink"]), "span":[0, RELEASE]}]
    paths = [strand_poses(art, i) for i in range(3)]
    for i, path in enumerate(paths):
        over = cubic_window(paths[(i - 1) % 3], .16*SEGMENTS, .52*SEGMENTS)
        cutter = stroke(over, width + 2 * art["crossing_gap"], [0,0,0,255])
        cutter["blend"] = "destination_out"
        graphics.append({"span":[RELEASE, FRAMES], "content":{"type":"group",
                         "children":[stroke(path, width, art["ink"]), cutter]}})
    return graphics


def make_paper(folder, paper):
    # One still, lightly shaded original paper field. Every moving edge is an N4 path.
    w, h = 192, 108
    pixels = bytearray()
    for y in range(h):
        for x in range(w):
            falloff = .83*((x/w-.42)**2 + .8*(y/h-.29)**2)
            pixels.extend(max(0, min(255, round(c - 10*falloff))) for c in paper)
    ppm = f"P6\n{w} {h}\n255\n".encode() + pixels
    subprocess.run(["ffmpeg","-nostdin","-v","error","-n","-f","image2pipe","-framerate",str(FPS),
                    "-vcodec","ppm","-i","pipe:0","-vf",
                    "loop=loop=-1:size=1:start=0,setpts=N/(30*TB),scale=out_color_matrix=bt709,"
                    "format=yuv420p,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                    "-frames:v",str(FRAMES),"-c:v","libx264","-g","1","-crf","13",
                    "-color_range","tv","-colorspace","bt709","-color_primaries","bt709","-color_trc","bt709",
                    str(folder/"paper.mp4")], input=ppm, check=True)


def make_sound(folder):
    """Three staggered harmonic plucks and the cadence ringing after the lock."""
    bus = array("f", [0]) * (FRAMES * SAMPLE_RATE // FPS)
    for frame, hz, amplitude, decay in [(17, 164.81, .17, .85), (25, 246.94, .11, .85),
            (32, 329.63, .09, .85), (55, 220, .08, 1.0), (87, 293.66, .10, 1.3),
            (111, 369.99, .08, 1.1), (140, 164.81, .14, 1.0), (146, 246.94, .09, .75),
            (152, 415.30, .08, .65)]:
        start = frame * SAMPLE_RATE // FPS
        length = min(len(bus)-start, int(3*decay*SAMPLE_RATE))
        for i in range(length):
            t = i/SAMPLE_RATE
            env = min(t/.012, 1) * math.exp(-3.8*t/decay) * min(1,(length-i)/(SAMPLE_RATE*.05))
            phase = TAU * hz * t
            bus[start+i] += amplitude * env * (math.sin(phase) + .29*math.sin(2.002*phase)*math.exp(-6*t)
                                               + .12*math.sin(4.007*phase)*math.exp(-9*t))
    samples = array("h", (round(32767 * s) for s in bus))
    if sys.byteorder != "little":
        samples.byteswap()
    with wave.open(str(folder/"score.wav"), "wb") as f:
        f.setparams((1,2,SAMPLE_RATE,0,"NONE",""))
        f.writeframes(samples.tobytes())


def build(folder, art):
    if not (-.20 <= art["seam_shift_turns"] <= .20 and abs(art["tip_pull_px"]) <= 175
            and -12 <= art["weave_delay_frames"] <= 12):
        raise ValueError("seam shift must be within ±0.20 turns, tip pull within ±175 px and weave delay within ±12 frames")
    if not 100 <= art["ring_radius"] <= 340 or not 1 <= art["stroke_width"] <= 70 or not 0 <= art["crossing_gap"] <= 30:
        raise ValueError("invalid ring radius, stroke width or crossing gap")
    graphics = layout(art)
    folder.parent.mkdir(parents=True, exist_ok=True)
    if folder.exists() and (not folder.is_dir() or any(folder.iterdir())):
        raise FileExistsError(f"choose an empty or new directory: {folder}")
    font = os.path.relpath(HERE/"fonts"/"N4GridSans.ttf", folder)
    project = {"size":[1920,1080], "fps":[FPS,1],
               "shots":[{"source":"paper.mp4","source_in":0,"duration":FRAMES}],
               "title":{"text":"","font":font,"font_size":16,"width":32,"position":[0,0],
                        "start":FRAMES,"fade_frames":0},
               "audio":[{"source":"score.wav","start_sample":0,"source_in_sample":0,
                         "samples":FRAMES*SAMPLE_RATE//FPS}],
               "graphics":graphics}
    # Publish picture, score and manifest together after all three are complete.
    with tempfile.TemporaryDirectory(dir=folder.parent) as scratch:
        staged = Path(scratch) / "project"
        staged.mkdir()
        make_paper(staged, art["paper"])
        make_sound(staged)
        (staged / "knot.json").write_text(project_text(project))
        staged.rename(folder)


def project_text(project):
    # Keep hierarchy and poses on separate lines, coordinates together in pairs.
    text = json.dumps(project, indent=2)
    return re.sub(r"\[\s*(-?\d[\d.]*\s*(?:,\s*-?\d[\d.]*\s*)+)\]",
                  lambda m: "[" + ", ".join(x.strip() for x in m[1].split(",")) + "]", text) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--art", type=Path, default=HERE/"knot-art.json", help="editable design settings")
    parser.add_argument("--seam-shift", type=float, help="move the seam shared by the first and second strands, in turns")
    parser.add_argument("--tip-pull", type=float, help="extra outward bend of the leading strand, in pixels")
    parser.add_argument("--weave-delay", type=int, help="retime the first strand's two weaving poses, in frames")
    args = parser.parse_args()
    art = json.loads(args.art.read_text())
    for key, val in (("seam_shift_turns",args.seam_shift), ("tip_pull_px",args.tip_pull),
                     ("weave_delay_frames",args.weave_delay)):
        if val is not None:
            art[key] = val
    build(args.output.resolve(), art)
