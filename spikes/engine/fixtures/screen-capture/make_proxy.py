"""Make a half-size draft derivative and verify every measured source PTS plus its terminal hold."""
import argparse
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def timeline(path):
    probe = json.loads(subprocess.check_output([
        'ffprobe','-v','error','-select_streams','v:0','-show_entries',
        'stream=width,height,time_base:frame=pts,duration','-of','json',str(path)]))
    stream = probe['streams'][0]
    frames = probe['frames']
    tb = Fraction(stream['time_base'])
    origin = frames[0]['pts']
    points = [(f['pts'] - origin) * tb for f in frames]
    last = frames[-1]['duration'] * tb
    if last <= 0:
        raise ValueError('missing final source hold')
    return points, last, [stream['width'], stream['height']]


def inspect(source, target, shifted):
    points, hold, size = timeline(source)
    derived, final, small = timeline(target)
    if len(points) != len(derived) or hold != final:
        raise ValueError('proxy changed the picture count or the terminal hold')
    differences = [(i,b-a) for i,(a,b) in enumerate(zip(points,derived)) if a!=b]
    expected = [(120,Fraction(1,1000))] if shifted else []
    if differences != expected:
        raise ValueError(f'unexpected source time differences: {differences}')
    return {'source_size':size,'proxy_size':small,'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),
            'proxy_sha256':hashlib.sha256(target.read_bytes()).hexdigest(),'pictures':len(points),
            'shifted_ordinals':[i for i,_ in differences], 'source_and_proxy_end_seconds':str(points[-1]+hold),
            'source_and_proxy_final_hold_seconds':str(hold)}


def make(source, target, shift):
    vf = 'scale=320:180:flags=area'
    if shift:
        vf = "setpts='PTS+if(eq(N,120),1,0)'," + vf
    with tempfile.TemporaryDirectory(dir=target.parent) as tmp:
        candidate=Path(tmp)/'draft.mp4'
        subprocess.run([
            'ffmpeg', '-nostdin', '-v', 'error', '-n', '-i', str(source), '-map', '0:v:0',
            '-vf', vf, '-fps_mode','passthrough','-enc_time_base','1:1000',
            '-video_track_timescale','1000','-c:v','libx264','-crf','21','-preset','fast',
            '-g','20','-bf','0','-color_range','tv','-colorspace','bt709','-color_trc','bt709',
            '-color_primaries','bt709','-an',str(candidate)
        ],check=True)
        report = inspect(source, candidate, shift)
        os.link(candidate, target)
        return report


if __name__ == '__main__':
    p=argparse.ArgumentParser()
    p.add_argument('source',type=Path)
    p.add_argument('target',type=Path)
    p.add_argument('--shift-one-onset',action='store_true',help='make a deliberate negative test')
    p.add_argument('--verify',action='store_true',help='inspect an existing derivative without rewriting it')
    a=p.parse_args()
    check = inspect if a.verify else make
    print(json.dumps(check(a.source,a.target,a.shift_one_onset),indent=2))
