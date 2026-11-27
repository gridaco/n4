"""Build the independent 1080p30/4K playback load in a new directory."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-xerror", *map(str, args)], check=True)


def prepare(dest):
    dest = dest.resolve()
    if dest.exists():
        raise SystemExit(f"Choose a new fixture directory: {dest}")
    with tempfile.TemporaryDirectory(dir=dest.parent) as stage:
        root = Path(stage) / "load"
        root.mkdir()
        # Author ten seconds of original movement, compress with fixed closed GOPs
        # and B frames, then repeat packets across a real two-minute PTS range.
        segment = Path(stage) / "segment.mp4"
        ffmpeg("-f", "lavfi", "-i", "testsrc2=s=480x270:r=30:d=10",
               "-vf", "scale=3840:2160:flags=bilinear,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
               "-filter_threads", "2", "-c:v", "libx264", "-threads", "4",
               "-preset", "ultrafast", "-crf", "26", "-pix_fmt", "yuv420p",
               "-x264-params", "keyint=75:min-keyint=75:scenecut=0:bframes=2:open-gop=0",
               "-video_track_timescale", "30000", segment)
        ffmpeg("-stream_loop", "11", "-i", segment, "-map", "0:v:0", "-an",
               "-c:v", "copy", "-movflags", "+faststart", root / "long-gop.mp4")
        # Distinct tracks with recurring tone bursts, aligned to 1-second boundaries.
        for name, freq, offset in [("voice-cue", 880, 0.0), ("bed-cue", 330, 0.5)]:
            expr = f"0.15*sin(2*PI*{freq}*t)*lt(mod(t+{offset}\\,1)\\,0.04)"
            ffmpeg("-f", "lavfi", "-i", f"aevalsrc={expr}:s=48000:d=120", "-ac", "2",
                   "-c:a", "pcm_s16le", root / f"{name}.wav")
        shutil.copyfile(Path(__file__).resolve().parents[1] / "fonts/N4GridSans.ttf", root / "font.ttf")
        project = {"size": [1920, 1080], "fps": [30, 1],
                   "shots": [{"source": "long-gop.mp4", "source_in": 0, "duration": 3600}],
                   "title": {"text": "Native playback", "font": "font.ttf", "font_size": 56,
                             "width": 750, "position": [64, 64], "start": 15, "fade_frames": 15},
                   "audio": [{"source": name + ".wav", "start_sample": 0, "source_in_sample": 0,
                              "samples": 5760000, "gain": gain}
                             for name, gain in [("voice-cue", 0.65), ("bed-cue", 0.28)]]}
        (root / "project.json").write_text(json.dumps(project, indent=2) + "\n")
        entries = "stream=codec_name,width,height,has_b_frames,nb_frames,time_base,avg_frame_rate,color_space,color_transfer,color_primaries"
        probe = subprocess.check_output(["ffprobe", "-v", "error", "-of", "json",
                                         "-show_entries", entries, root / "long-gop.mp4"])
        stream = json.loads(probe)["streams"][0]
        if not (int(stream["nb_frames"]) == 3600 and stream["has_b_frames"] >= 1 and stream["width"] == 3840
                and stream["height"] == 2160 and stream["color_transfer"] == "bt709"):
            raise ValueError(f"Unexpected workload source: {stream}")
        sums = {}
        for file in sorted(root.iterdir()):
            with file.open("rb") as inp:
                digest = hashlib.sha256()
                for block in iter(lambda: inp.read(1024 * 1024), b""):
                    digest.update(block)
                sums[file.name] = digest.hexdigest()
        receipt = {"video": stream, "sha256": sums,
                   "ffmpeg": subprocess.check_output(["ffmpeg", "-version"], text=True).splitlines()[0],
                   "recipe": "Original generated testsrc2 movement. 12 closed-GOP ten-second sections, two independent 120s tone-burst WAVs."}
        (root / "fixture-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        root.rename(dest)
    print(dest / "project.json")


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output", type=Path)
    prepare(cli.parse_args().output)
