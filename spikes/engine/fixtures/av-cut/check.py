"""Inspect the delivered MP4 independently using the source marks and the decoded soundtrack."""
import argparse
from fractions import Fraction
import json
from pathlib import Path
import subprocess
import wave

import numpy as np
from compare_preview import check_preview

HERE = Path(__file__).resolve().parent


def probe(path, selector, scope, entries):
    return json.loads(subprocess.check_output([
        'ffprobe', '-v', 'error', '-select_streams', selector, '-show_entries', entries,
        '-of', 'json', str(path)]))[scope]


def frame_bytes(stream, size):
    data = stream.read(size)
    if len(data) != size:
        raise ValueError(f'short decoder frame, received {len(data)} / {size} bytes')
    return data


def check_picture(path):
    v = probe(path, 'v:0', 'streams',
              'stream=codec_name,width,height,pix_fmt,color_range,color_transfer,color_space,'
              'color_primaries,time_base,duration_ts,nb_frames')[0]
    assert (v['codec_name'], v['width'], v['height'], v['pix_fmt'], v['color_range']) == (
        'h264',640,360,'yuv420p','tv'), v
    assert [v[k] for k in ('color_transfer','color_space','color_primaries')] == ['bt709']*3
    timebase = Fraction(v['time_base'])
    pts = probe(path,'v:0','frames','frame=best_effort_timestamp')
    assert len(pts) == 300 and Fraction(v['duration_ts'])*timebase == 10
    assert all(p['best_effort_timestamp'] * timebase == Fraction(i,30) for i,p in enumerate(pts))

    expected_flashes = [24,150,267]
    flashes, cuts, gray = [], [], []
    with subprocess.Popen(['ffmpeg','-v','error','-i',str(path),'-map','0:v:0',
                       '-fps_mode','passthrough','-pix_fmt','yuv420p','-f','rawvideo','pipe:1'],
                       stdout=subprocess.PIPE) as decoder:
        for timeline in range(300):
            raw = frame_bytes(decoder.stdout,640*360*3//2)
            y = np.frombuffer(raw, dtype=np.uint8, count=640*360).reshape(360,640)
            source = sum(1 << bit for bit in range(9) if y[324,32+bit*13] > 125)
            expected = (12 if timeline < 150 else 30) + timeline
            assert source == expected, (timeline,source,expected)
            if timeline in (149,150):
                cuts.append({'timeline':timeline,'source':source})
            if y[312,599] > 175:
                flashes.append(timeline)
            gray.append(int(y[325,169]))
        assert decoder.stdout.read(1) == b'' and decoder.wait() == 0
    assert flashes == expected_flashes, flashes
    # sRGB 128 -> linear light -> BT.709 transfer -> 16..235 yields luma code 115.
    assert max(abs(value-115) for value in gray) <= 3, [min(gray),max(gray)]
    return {'frames':300,'duration_seconds':10,'cut':cuts, 'flash_frames':flashes,
            'neutral_patch_luma_range': [min(gray),max(gray)], 'video':v}


def read_wav(path):
    with wave.open(str(path)) as wav:
        assert wav.getframerate() == 48000 and wav.getnchannels() == 2 and wav.getsampwidth()==2
        return np.frombuffer(wav.readframes(wav.getnframes()),'<i2').astype(float).reshape(-1,2)/32768


def check_sound(path, source_dir):
    a = probe(path, 'a:0', 'streams','stream=codec_name,channels,sample_rate,start_pts,duration_ts,time_base')[0]
    assert (a['codec_name'],a['channels'],a['sample_rate'],a['start_pts']) == ('aac',2,'48000',0), a
    assert a['duration_ts'] * Fraction(a['time_base']) == 10, a
    data = subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-map','0:a:0',
        '-c:a','pcm_f32le','-f','f32le','pipe:1'])
    signal = np.frombuffer(data, '<f4').reshape(-1,2)
    assert len(signal) >= 480000
    cues = read_wav(source_dir/'cues.wav')
    lead = read_wav(source_dir/'lead.wav')[2377:,0]
    marks = []
    for frame in (24,150,267):
        sample = frame * 1600
        ref = cues[1234+sample:1234+sample+960,1]*0.8
        window = signal[sample-1600:sample+960+1600,1]
        score = np.correlate(window,ref,'valid')
        lag = int(score.argmax()) - 1600
        decoded = window[lag+1600:lag+1600+960]
        corr = float(np.corrcoef(ref, decoded)[0,1])
        assert abs(lag) <= 24 and corr >= 0.97, (lag,corr)
        assert abs(float(np.linalg.norm(decoded)/np.linalg.norm(ref)) - 1) < 0.1
        marks.append({'frame':frame,'timeline_sample':sample, 'error_samples':lag,'correlation':round(corr,6)})
    start = 180000
    decoded = signal[start:start+len(lead),0]
    ref = lead*0.9
    corr = float(np.corrcoef(ref,decoded)[0,1])
    assert corr > 0.97 and abs(np.linalg.norm(decoded)/np.linalg.norm(ref)-1) < 0.1, corr
    assert max(abs(signal[:start-1024,0])) < 0.02
    threshold = np.where(abs(decoded)>0.02)[0]
    assert len(threshold) > 0
    active = [int(start+threshold[0]),int(start+threshold[-1])]
    assert active[0] < 240000 < active[1]
    return {'sample_rate':48000,'presentation_samples':480000,'decoder_output_samples':len(signal),
            'marks':marks, 'lead_active_sample_range':active,'lead_correlation':round(corr,6)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('movie',type=Path)
    parser.add_argument('--project',type=Path,default=HERE/'edit.json')
    parser.add_argument('--preview-cli',type=Path,help='also compare every inspection PNG to the encoder input digest')
    parser.add_argument('--report',type=Path,help='JSON stdout of n4 export')
    args = parser.parse_args()
    out = {'picture':check_picture(args.movie),'sound':check_sound(args.movie,args.project.parent)}
    if args.preview_cli:
        assert args.report
        out['parity'] = check_preview(args.preview_cli.resolve(),args.project,args.report)
    print(json.dumps(out,indent=2))


if __name__ == '__main__':
    main()
