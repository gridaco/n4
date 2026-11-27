"""Compare native 30 fps seek requests to the original captured browser images and times."""
import argparse
from bisect import bisect_right
from fractions import Fraction
import json
from pathlib import Path
import subprocess
import tempfile

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent


def main(cli,root):
    capture=json.loads((root/'capture.json').read_text())
    originals=capture['pictures']
    records=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0',
        '-show_entries','stream=codec_name,time_base:frame=pts,key_frame,pict_type','-of','json',
        str(root/'recording.mp4')]))
    frames=records['frames']; stream=records['streams'][0]
    tb=Fraction(stream['time_base'])
    assert len(originals)==len(frames)
    assert all(rec['pts']*tb*1000==point[0] for point,rec in zip(originals,frames))
    milliseconds=[p[0] for p in originals]
    intervals=np.diff(milliseconds)
    assert len(set(intervals))>=3 and max(intervals)>=1000
    assert any(f['pict_type']=='B' for f in frames)
    shots=[(0,237,900),(900,3620,720)]
    requests=[0,899,900,901,1619,899,1,1600,32]
    # Around exact source onsets, request the project frames immediately before and after.
    for start,tick,count in shots:
        changes=[m for m in milliseconds if tick < m*30/1000 < tick+count]
        for m in changes[::max(1,len(changes)//20)]:
            at=(m*30+999)//1000-tick+start
            requests.extend([at,at-1,at])
    # Include unique uncompressed source images retained across the recording.
    for p in originals:
        if len(p)>2:
            for start,tick,count in shots:
                at=(p[0]*30+999)//1000
                if tick<=at<tick+count: requests.append(start+at-tick)
    def expected(frame):
        start,tick,count=shots[0] if frame<900 else shots[1]
        source_tick=frame-start+tick
        return bisect_right(milliseconds,Fraction(source_tick*1000,30))-1
    with tempfile.TemporaryDirectory() as temp:
        out=Path(temp)/'inspection'
        subprocess.run([str(cli),'render',str(root/'seek.json'),'--frames',','.join(map(str,requests)),
                        '--out',str(out)],check=True,stdout=subprocess.DEVNULL)
        report=json.loads((out/'receipt.json').read_text())
        pairs=[]; refs={}
        for i,(request,receipt) in enumerate(zip(requests,report['frames'])):
            ordinal=expected(request)
            p=originals[ordinal]
            pic=np.asarray(Image.open(out/receipt['file']).convert('RGB'))
            marker=sum(1<<bit for bit in range(12) if pic[326,37+bit*18,0]>127)
            assert marker==p[1],(request,ordinal,marker,p[1])
            if len(p)>2:
                ref=np.asarray(Image.open(root/'references'/p[2]).convert('RGB'))
                mae=float(np.mean(np.abs(ref.astype(float)-pic)))
                assert mae < 2, (ordinal,mae)
                refs[p[2]]=round(mae,3)
            pairs.append([request,ordinal,marker])
        assert len(pairs)==len(requests) and len(refs)>=2
    return {'browser':capture['browser'],'duration_between_first_and_last_frame_ms':milliseconds[-1],
        'recorded_frames':len(frames),'distinct_interval_ms':len(set(intervals)),'interval_ms_min_max':
        [int(min(intervals)),int(max(intervals))], 'keyframes':sum(f['key_frame'] for f in frames),
        'b_frames':sum(f['pict_type']=='B' for f in frames),'all_muxed_pts_equal_measured_capture_pts':True,
        'timeline_seeks_checked':len(requests),'distinct_frames_requested':len(set(requests)),
        'trim_and_backward_examples':pairs[:9],'rgb_mean_absolute_error_against_browser_png':refs}


if __name__=='__main__':
    p=argparse.ArgumentParser()
    p.add_argument('cli',type=Path)
    p.add_argument('--fixture',type=Path,default=ROOT)
    args=p.parse_args()
    print(json.dumps(main(args.cli.resolve(),args.fixture),indent=2))
