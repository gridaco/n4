"""Generate the recorder's original video signal and the film's stereo soundtrack."""
from pathlib import Path
import subprocess
import wave
import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
FPS, SR = 24, 48000
DURATION = 144


def signal(frame):
    t = frame / FPS
    im = Image.new("RGB", (640, 360), "#172124")
    draw = ImageDraw.Draw(im)
    x0, x1, y0, y1 = 28, 612, 106, 289
    for x in np.linspace(x0, x1, 25):
        draw.line((x, y0, x, y1), fill="#38494a")
    for y in np.linspace(y0, y1, 9):
        draw.line((x0, y, x1, y), fill="#38494a")
    x = np.linspace(0, 1, x1 - x0 + 1)
    center = (y0 + y1) / 2
    phase = 2 * np.pi * (3.1 * x - 1.15 * t)
    envelope = 0.46 + 0.38 * np.cos(2 * np.pi * (.68*x - .16*t))
    curve = np.sin(phase + .7 * np.sin(phase * 1.55 + t)) * envelope
    y = center - 63 * curve
    for row, color, width in [(3, "#513a29", 8),(0, "#f48c44", 4),(-1, "#ffeab8", 1)]:
        draw.line(list(zip(x * (x1 - x0) + x0, y + row)), fill=color, width=width)
    cursor = x0 + (t * 117) % (x1 - x0)
    draw.line((cursor, y0, cursor, y1), fill="#e4bea2", width=2)
    for k in range(48):
        act = k < int(29 + 15 * np.sin(t * 3.5) ** 2)
        draw.rectangle((28 + k*9, 316, 33 + k*9, 332), fill="#e38a46" if act else "#354547")
    draw.ellipse((590, 35, 607, 52), fill="#f39c62")
    return im


def video():
    target = ROOT / "assets/signal.mp4"
    cmd = ["ffmpeg","-hide_banner","-loglevel","error","-y","-f","rawvideo","-pix_fmt","rgb24",
           "-s","640x360","-r",str(FPS),"-i","-","-an","-c:v","libx264","-preset","medium",
           "-crf","14","-g","12","-vf","scale=out_color_matrix=bt709:out_range=tv","-pix_fmt","yuv420p","-color_primaries","bt709",
           "-color_trc","bt709","-colorspace","bt709","-bsf:v",
           "h264_metadata=colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1:video_full_range_flag=0","-movflags","+faststart",str(target)]
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as p:
        for frame in range(DURATION + 12):
            p.stdin.write(signal(frame).tobytes())
        p.stdin.close()
        if p.wait():
            raise RuntimeError("signal encode failed")


def score():
    t = np.arange(SR * DURATION // FPS) / SR
    rng = np.random.default_rng(5471)
    n = len(t)
    out = np.zeros((n, 2))
    # Recorded envelope of the monitor opening: a low pulse, sparse switching clicks,
    # and a small swelled triad arriving with the last wide shot.
    for freq, gain, phase in [(55,.10,.2),(82.4069,.05,.0),(164.814,.018,.6)]:
        bed = np.sin(2*np.pi*freq*t + .07*np.sin(2*np.pi*.26*t)+phase)
        envelope=np.minimum(t/.38,1)*np.clip((6-t)/.9,0,1) * (.8+.18*np.cos(t*.8))
        out += (gain * bed * envelope)[:,None]
    for start,pitch,level in [(0.02,173,.3),(1.45,311,.16),(2.08,233,.36),(4.22,173,.36),(4.44,415,.12)]:
        u=t-start
        active=u>=0
        local=u[active]
        ring=np.sin(2*np.pi*pitch*local+2.3*np.exp(-local*24))*np.exp(-local*14)
        click=rng.normal(0,.4,len(local))*np.exp(-local*110)
        hit=(ring+click)*np.minimum(local/.0015,1)*level
        out[active] += np.stack((hit, hit*.9),axis=-1)
    for k,freq in enumerate([220,329.628,391.995]):
        u=np.maximum(t-4.05-k*.10,0)
        out[:,k%2] += .034*np.sin(2*np.pi*freq*u)*np.minimum(u/.14,1)*np.exp(-1.7*u)*np.minimum((6-t)/.32,1)
    with wave.open(str(ROOT/"assets/score.wav"),"wb") as f:
        f.setparams((2,2,SR,0,"NONE","not compressed"))
        f.writeframes((np.clip(out,-1,1)*32767).astype("<i2").tobytes())


if __name__ == "__main__":
    video()
    score()
