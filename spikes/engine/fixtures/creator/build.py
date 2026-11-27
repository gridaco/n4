"""Build independent source plates, WAV stems, SVG paths and four editable test cuts."""
import argparse
import json
import math
import subprocess
import wave
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
W, H, FPS, SR = 960, 540, 30, 48_000
FONT = "../fonts/N4GridSans.ttf"
PRODUCT_FRAMES, CHART_FRAMES = 180, 225


def font(px):
    return ImageFont.truetype(str(HERE / FONT), px)


def label(value, x, y, size, width, rgb):
    return {"content": {"type": "label", "text": value, "font": FONT, "size": size,
                        "width": width, "rgb": rgb}, "position": [x, y]}


def svg_asset(name, position=None, **fields):
    return {"content": {"type": "svg", "source": name}, "position": position or [0, 0], **fields}


def write_json(name, value):
    (HERE / name).write_text(json.dumps(value, indent=2) + "\n")


def write_svg(name, width, height, paths):
    (HERE / name).write_text(f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">\n'
                             + "\n".join("  " + element for element in paths) + "\n</svg>\n")


def stroke(d, color, width):
    return f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{width}" stroke-linejoin="round" stroke-linecap="round"/>'


def encode(name, images):
    # Convert display-referred sRGB artwork into tagged, limited-range 709 footage.
    cmd = ["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-f", "rawvideo",
           "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
           "-vf", "scale=in_range=full:out_color_matrix=bt709:out_range=full,format=yuv444p12,colorspace=iall=bt709:itrc=srgb:irange=pc:all=bt709:range=tv:format=yuv420p",
           "-c:v", "libx264", "-threads", "2", "-preset", "fast", "-crf", "15", "-g", "60",
           "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv",
           "-movflags", "+faststart", str(HERE / name)]
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as proc:
        for image in images:
            proc.stdin.write(image.convert("RGB").tobytes())
        proc.stdin.close()
        if proc.wait():
            raise RuntimeError(f"encode failed for {name}")


def audio(name, samples):
    wave_data = np.asarray(samples * 32767, dtype="<i2")
    with wave.open(str(HERE / name), "wb") as out:
        out.setparams((1, 2, SR, 0, "NONE", "not compressed"))
        out.writeframes(wave_data.tobytes())


def clip(name, gain=1.0, start=0):
    with wave.open(str(HERE / name)) as stem:
        assert stem.getframerate() == SR
        count = stem.getnframes()
    return {"source": name, "start_sample": start, "source_in_sample": 0, "samples": count, "gain": gain}


def paper(base, seed):
    rng = np.random.default_rng(seed)
    noise = rng.normal(0, 1.5, (H, W)) + 2 * np.sin(np.arange(W) / 210)
    return Image.fromarray(np.uint8(np.clip(np.asarray(base) + noise[:, :, None], 0, 255)))


def create_chart():
    plot_width, plot_height = 786, 206
    x0, y0 = 109, 172
    ivory, muted = [233, 230, 207], [161, 166, 143]
    step_to_x = lambda step: round(3 + step / 6 * (plot_width - 6), 2)
    depth_to_y = lambda level: round(3 + (420 - level) / 120 * (plot_height - 6), 2)
    axis = []
    annotations = [label("FIELD NOTES   /   SIGNAL", 56, 29, 15, 460, [216, 218, 122]),
                   label("Synthetic sounding levels, arbitrary units", 57, 115, 18, 750, ivory)]
    for level in (300, 340, 380, 420):
        y = depth_to_y(level)
        axis.append(stroke(f"M3 {y} H{plot_width-3}", "#adb391", 2))
        annotations.append(label(str(level), 58, round(y0 + y - 10), 16, 48, muted))
    for step in (0, 2, 4, 6):
        x = step_to_x(step)
        axis.append(stroke(f"M{x} {plot_height-4} v10", "#adb391", 2))
        annotations.append(label(str(step), round(x0 + x - 18), y0 + plot_height + 11, 16, 62, ivory))
    levels = [307, 322, 333, 357, 367, 395, 411]
    path = " ".join(("M" if i == 0 else "L") + f"{step_to_x(i)} {depth_to_y(level)}"
                    for i, level in enumerate(levels))
    write_svg("axes.svg", plot_width, plot_height + 10, axis)
    write_svg("series.svg", plot_width, plot_height, [stroke(path, "#edff6c", 7)])
    texture = paper((34, 40, 32), 2020)
    encode("paper.mp4", (texture for _ in range(CHART_FRAMES)))
    t = np.arange(CHART_FRAMES * SR // FPS) / SR
    notes = np.zeros_like(t)
    for when, freq in ((0, 196), (2.0, 246.94), (4.0, 293.66), (6, 329.63)):
        u = np.maximum(0, t - when)
        notes += .1 * (1 - np.exp(-u * 17)) * np.exp(-u * 1.2) * np.sin(2 * np.pi * freq * u)
    audio("chart-bed.wav", notes)
    u = np.arange(6 * SR) / SR
    audio("chart-lead.wav", 0.12 * np.sin(2*np.pi*147*u + 2*np.sin(2*np.pi*1.5*u))
          * np.square(np.sin(np.pi * u / 6)))
    for revision in (False, True):
        delay = 6 if revision else 0
        axes = svg_asset("axes.svg", [x0, y0], filter={"shader": "dry-ink.wgsl", "amount": 82 if revision else 16})
        series = svg_asset("series.svg", [x0, y0], wipe=[18 + delay, 171 + delay])
        layers = [axes, series, *annotations,
                  label("Source   Synthetic measurements for the N4 timing fixture",
                        56, 470, 14, 850, muted),
                  label("PASS 0 - 6     /     SOUNDING SAMPLES", 57, 436, 15, 680, ivory)]
        edit = project("paper.mp4", CHART_FRAMES, "The sounding line", [54, 53], 52, 850, layers)
        edit["audio"] = [clip("chart-lead.wav", 0.9, 14_400), clip("chart-bed.wav", 0.5)]
        write_json("chart-revised.json" if revision else "chart.json", edit)


def project(plate, frames, title, position, size, width, layers):
    return {"size": [W, H], "fps": [FPS, 1], "shots": [{"source": plate, "source_in": 0, "duration": frames}],
            "title": {"text": title, "font": FONT, "font_size": size, "position": position, "width": width,
                      "start": 0, "fade_frames": 10}, "graphics": layers}


def product_plate(click):
    base = paper((40, 44, 67), 42)
    d = ImageDraw.Draw(base)
    # Sample application surface. Native editor elements stay in the authored project.
    d.rounded_rectangle((380, 59, 916, 476), radius=27, fill="#dbe0f3")
    d.text((411, 83), "SIDEBAND", font=font(18), fill="#333852", stroke_width=1)
    d.line((410, 121, 887, 121), fill="#a8aec8", width=1)
    d.text((411, 140), "Studio take", font=font(36), fill="#22263b", stroke_width=1)
    d.text((413, 190), "room-16.wav   /   48 kHz", font=font(15), fill="#4b5067")
    d.rounded_rectangle((411, 229, 885, 354), radius=13, fill="#2b304a")
    d.line((411, 374, 885, 374), fill="#a8aec8", width=1)
    d.text((412, 399), "Remove hum", font=font(22), fill="#272c43")
    d.text((412, 435), "60 Hz + harmonics", font=font(14), fill="#595e75")
    for frame in range(PRODUCT_FRAMES):
        image = base.copy()
        draw = ImageDraw.Draw(image)
        clean = frame >= click
        for i in range(149):
            wave_x = i / 148
            envelope = 0.1 + abs(math.sin(wave_x * 14 + .2) * math.cos(wave_x * 26)) ** .7
            height = round(3 + 43 * envelope * (.56 + .44 * abs(math.sin(i * 2.1))))
            if not clean: height += int(6 + 5 * math.cos(i * 2.3))
            draw.line((426 + i * 3, 291 - height, 426 + i * 3, 291 + height),
                      fill="#b8b9ff" if not clean else "#b1e7c5", width=2)
        play_x = round(425 + (frame / (PRODUCT_FRAMES - 1)) * 445)
        draw.line((play_x, 234, play_x, 349), fill="#ffffff", width=1)
        draw.rounded_rectangle((805, 398, 880, 436), radius=20, fill="#345c50" if clean else "#9b9fb4")
        button_x = 860 if clean else 825
        draw.ellipse((button_x-14, 403, button_x+14, 431), fill="#f5f5fc")
        u = min(1, max(0, (frame - click + 22) / 22))
        eased = 1 - (1-u) ** 3
        x, y = 744 + 101 * eased, 337 + 80 * eased
        draw.polygon([(x,y),(x+4,y+22),(x+9,y+16),(x+17,y+24),(x+20,y+21),(x+13,y+13),(x+21,y+11)],
                     fill="#fafaff", outline="#33354e", width=2)
        if 0 <= frame - click < 10:
            radius = 12 + (frame - click) * 2
            draw.ellipse((x-radius,y-radius,x+radius,y+radius), outline="#7c79ba", width=2)
        yield image


def create_product():
    for revision, click in [(False, 48), (True, 78)]:
        take = "take-b" if revision else "take-a"
        encode(take + ".mp4", product_plate(click))
        t = np.arange(PRODUCT_FRAMES * SR // FPS) / SR
        hum = .15 * np.sin(2*np.pi*60*t) + .10 * np.sin(2*np.pi*180*t)
        hum *= np.clip((click / FPS + .01 - t) * 100, 0, 1)
        signal = np.zeros_like(t)
        for when, freq in [(0, 392), (.75, 329.63), (1.5, 440), (2.25, 392), (3, 329.63), (3.75, 440), (4.5, 523.25), (5.25, 392)]:
            w = np.maximum(0, t-when)
            signal += .24 * np.sin(2*np.pi*freq*w) * (1 - np.exp(-w*180)) * np.exp(-w*4)
        audio(take + ".wav", hum + signal)
        layers = [label("SIDEBAND   /   AUDIO TOOLS", 43, 74, 14, 320, [202, 204, 250]),
                  label("Remove electrical hum from a recording in one switch.",
                        43, 364, 22, 280, [215, 218, 234])]
        title = "Hear the take." if not revision else "Hear the take without the hum."
        edit = project(take + ".mp4", PRODUCT_FRAMES, title, [39, 151], 52, 330, layers)
        edit["audio"] = [clip(take + ".wav", .9)]
        write_json("product-revised.json" if revision else "product.json", edit)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("demo", choices=("chart", "product", "both"), default="both", nargs="?")
    choice = parser.parse_args().demo
    if choice in ("chart", "both"): create_chart()
    if choice in ("product", "both"): create_product()
