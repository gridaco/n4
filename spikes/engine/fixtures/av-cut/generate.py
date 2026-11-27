"""Generate our two technical plates and isolated tone/cue stems. No downloaded media."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import wave

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent
RATE = 48000
FPS = 30
SIZE = (640, 360)
FONT = ROOT.parent / 'fonts' / 'N4GridSans.ttf'
FLASH_IDS = (36, 180, 297)


def image(i):
    second = i >= 180
    ink = '#ffb354' if second else '#7be0d8'
    image = Image.new('RGB', SIZE, '#152932' if second else '#182132')
    draw = ImageDraw.Draw(image)
    draw.text((28, 22), 'N4   /   MOTION STUDY', font=small, fill='#e8e7df')
    draw.text((510, 22), f'SOURCE  {i:03d}', font=small, fill='#e8e7df')
    draw.line((28, 49, 612, 49), fill='#53616c', width=1)
    for x in range(29, 612, 32):
        draw.line((x, 177, x, 263), fill='#354453')
    last = None
    phase = i * 0.055
    for x in range(29, 612, 2):
        amp = 15 + 21 * (0.5 + 0.5 * math.cos(x / 79 - phase))
        y = round(220 + amp * math.sin(x / 58 + phase))
        if last:
            draw.line((last, (x, y)), fill=ink, width=3)
        last = (x, y)
    x = 32 + i * 3 % 575
    draw.line((x, 177, x, 264), fill='#e8e7df', width=2)
    draw.text((28, 275), '02   /   AFTER THE CUT' if second else '01   /   BEFORE THE CUT',
              font=small, fill=ink)
    # End-to-end oracles lie outside the native title and have ample chroma margins.
    for bit in range(9):
        on = i & (1 << bit)
        draw.rectangle((28 + bit*13, 320, 37 + bit*13, 329), fill='white' if on else 'black')
    draw.rectangle((160, 318, 178, 332), fill='#808080')
    flash = i in FLASH_IDS
    draw.ellipse((586, 300, 612, 326), fill='#ffffff' if flash else '#121212')
    return image


def write_wave(path, samples):
    with wave.open(str(path), 'wb') as out:
        out.setparams((2, 2, RATE, 0, 'NONE', 'not compressed'))
        out.writeframes((samples * 32767).clip(-32768, 32767).astype('<i2').tobytes())


def generate(dir_):
    dir_.mkdir(parents=True, exist_ok=True)
    with subprocess.Popen([
        'ffmpeg', '-y', '-nostdin', '-v', 'error', '-f', 'rawvideo', '-pixel_format', 'rgb24',
        '-video_size', '640x360', '-framerate', '30', '-i', 'pipe:0',
        '-c:v', 'ffv1', '-level', '3', '-g', '15', '-threads:v', '1', '-pix_fmt', 'bgr0',
        '-color_primaries', 'bt709', '-color_trc', 'iec61966-2-1', '-colorspace', 'rgb',
        '-color_range', 'pc', '-flags:v', '+bitexact', '-fflags', '+bitexact',
        str(dir_ / 'scope.mkv')], stdin=subprocess.PIPE) as process:
        with process.stdin as stream:
            for i in range(330):
                stream.write(image(i).tobytes())
        if process.wait():
            raise RuntimeError('source plate encoding failed')

    samples = 10 * RATE
    head = 1234
    cue = np.zeros((samples + head, 2))
    # A short chirp has one unambiguous correlation peak even after perceptual coding.
    t = np.arange(960) / RATE
    click = 0.75*np.hanning(len(t))*np.sin(2*np.pi*(450*t + 0.5*125000*t*t))
    for frame in (24,150,267):
        i = head + frame * (RATE//FPS)
        cue[i:i+len(t), 1] = click
    write_wave(dir_/'cues.wav', cue)

    # A three-second lead-in begins before the editorial cut and continues across it.
    t = np.arange(3 * RATE) / RATE
    envelope = (1 - np.exp(-18 * t)) * (1 - np.exp(-18 * (3 - t)))
    lead = 0.27 * envelope * np.sin(2 * np.pi * (144 * t + 7 * t * t))
    stem = np.zeros((2377 + len(lead), 2))
    stem[2377:, 0] = lead
    write_wave(dir_ / 'lead.wav', stem)
    project = {
        'size':[640,360], 'fps':[30,1],
        'shots':[{'source':'scope.mkv','source_in':12,'duration':150},
                 {'source':'scope.mkv','source_in':180,'duration':150}],
        'title':{'text':'A cut you can hear.', 'font':os.path.relpath(FONT, dir_), 'font_size':39,
                 'width':580,'position':[28,83],'start':5,'fade_frames':12},
        'audio':[{'source':'lead.wav','start_sample':180000,'source_in_sample':2377,'samples':len(lead),'gain':0.9},
                 {'source':'cues.wav','start_sample':0,'source_in_sample':head,'samples':samples,'gain':0.8}]}
    (dir_/'edit.json').write_text(json.dumps(project, indent=2)+'\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, default=ROOT)
    args = parser.parse_args()
    small = ImageFont.truetype(str(FONT), 13)
    generate(args.out.resolve())
