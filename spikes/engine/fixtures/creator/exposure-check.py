"""Build and verify a 20-frame exposure study with a held input and a hard cut."""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent


def build():
    path = HERE / "cut-markers.mkv"
    command = ["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24",
               "-s", "384x180", "-r", "30", "-i", "-", "-c:v", "ffv1", "-pix_fmt", "bgr0",
               "-color_trc", "iec61966-2-1", "-color_primaries", "bt709", "-colorspace", "rgb",
               "-color_range", "pc", "-g", "1", "-threads", "1", str(path)]
    with subprocess.Popen(command, stdin=subprocess.PIPE) as encoder:
        for frame in range(24):
            plate = Image.new("RGB", (384,180), "black" if frame < 12 else (28, 58, 44))
            draw = ImageDraw.Draw(plate)
            for bit in range(5):
                if frame & (1 << bit):
                    draw.rectangle((16+12*bit,157,25+12*bit,166), fill="white")
            encoder.stdin.write(plate.tobytes())
        encoder.stdin.close()
        if encoder.wait(): raise RuntimeError("plate encode failed")
    (HERE / "exposure-bar.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="256" height="40">\n'
        '  <path d="M0 0H256V40H0Z" fill="#ffffff"/>\n</svg>\n')
    edit = {"size": [384,180], "fps": [30,1], "shots": [
                {"source":"cut-markers.mkv","source_in":0,"duration":11},
                {"source":"cut-markers.mkv","source_in":10,"duration":1},
                {"source":"cut-markers.mkv","source_in":14,"duration":1},
                {"source":"cut-markers.mkv","source_in":14,"duration":7}],
            "title":{"text":"Four samples / one picture", "font":"../fonts/N4GridSans.ttf", "font_size":23,
                     "width":350,"position":[15,12],"start":0,"fade_frames":0},
            "graphics":[{"content":{"type":"svg","source":"exposure-bar.svg"}, "position":[64,81], "wipe":[8,9]}],
            "exposure":{"samples":4,"angle_degrees":360}}
    (HERE / "exposure.json").write_text(json.dumps(edit, indent=2) + "\n")


def check(cli, out):
    import importlib.util
    proxy = importlib.util.spec_from_file_location("compare_preview", HERE.parent / "av-cut" / "compare_preview.py")
    parity = importlib.util.module_from_spec(proxy)
    proxy.loader.exec_module(parity)
    out.mkdir(parents=True, exist_ok=True)
    movie = out / "exposure.mp4"
    edit = HERE / "exposure.json"
    report = out / "exposure-export.json"
    result = subprocess.check_output([cli,"export",edit,"--out",movie], text=True)
    report.write_text(result)
    preview = parity.check_preview(cli, edit, report)
    with tempfile.TemporaryDirectory() as temp:
        frames = Path(temp) / "frames"
        subprocess.run([cli,"render",edit,"--frames","7,8,9,10,11,12,13","--out",frames],check=True)
        expected_ids = [7,8,9,10,10,14,14]
        levels = []
        for row, (frame, expected_id) in enumerate(zip(range(7,14),expected_ids)):
            image = Image.open(frames / f"{row:03}-f{frame:04}.png")
            assert all(image.getpixel((20+12*b,162))[0] == (255 if expected_id & (1<<b) else (0 if frame < 12 else 28)) for b in range(5))
            assert image.getpixel((375,110)) == ((0,0,0,255) if frame<12 else (28,58,44,255))
            if frame == 8:
                # 4 midpoint samples reveal 32,96,160,224 pixels. Source white, black background.
                levels = [image.getpixel((64+x,99))[0] for x in (0,40,103,170,230)]
                assert all(abs(a-b)<=1 for a,b in zip(levels,[255,225,188,137,0])), levels
                image.save(out / "exposure.png")
        assert image.getpixel((100,100))[0] == 255
    raw = subprocess.check_output(["ffmpeg","-v","error","-i",movie,"-map","0:v:0","-vf",
          "colorspace=iall=bt709:irange=tv:all=bt709:trc=srgb:range=pc:format=yuv444p12,scale=in_color_matrix=bt709:in_range=full:out_range=full,format=rgb24",
          "-f","rawvideo","pipe:1"])
    assert len(raw) == 20*384*180*3
    def pixel(frame, x, y): return raw[3*((frame*180+y)*384+x)]
    assert all(abs(pixel(8,64+x,99)-sample)<=4 for x,sample in zip((0,40,103,170,230),levels))
    ids = [sum((pixel(frame,20+12*b,162)>127) << b for b in range(5)) for frame in range(7,14)]
    assert ids == expected_ids
    preview.update({"linear_light_gray_samples_srgb":levels, "decoded_cut_source_ids_frames_7_to_13":ids})
    (out / "exposure-checks.json").write_text(json.dumps(preview, indent=2)+"\n")


if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--cli",type=Path)
    parser.add_argument("--out",type=Path)
    options=parser.parse_args()
    if options.build: build()
    if options.cli: check(options.cli.resolve(), options.out.resolve())
