"""Preserve actual compositor presentation intervals and a browser-side image oracle."""
from pathlib import Path
import argparse
import json
import subprocess
import tempfile
from PIL import Image


def marker(image):
    return sum(1<<bit for bit in range(12) if image.getpixel((37+18*bit,326))[0]>127)


def package(raw, destination):
    capture = json.loads((raw/'capture.json').read_text())
    frames = capture.pop('frames')
    assert len(frames)>1 and frames[0]['ms']==0
    destination.mkdir(parents=True,exist_ok=True)
    oracle = []
    selected = {int(i*(len(frames)-1)/11) for i in range(12)}
    references = destination/'references'
    references.mkdir(exist_ok=True)
    manifest = ['ffconcat version 1.0']
    for i,f in enumerate(frames):
        png=raw/f['file']
        assert "'" not in str(png.resolve())
        manifest.extend([f"file '{png.resolve()}'", "option framerate 1000"])
        if i+1<len(frames):
            duration = frames[i+1]['ms']-f['ms']
            assert duration>0
            manifest.append(f'duration {duration/1000:.3f}')
        with Image.open(png) as original:
            assert original.width>=640 and original.height>=360
            image=original.convert('RGB').crop((0,0,640,360))
            record=[f['ms'], marker(image)]
            if i in selected:
                name=f'capture-{i:04}.png'
                image.save(references/name)
                record.append(name)
            oracle.append(record)
    with tempfile.TemporaryDirectory() as tmp:
        listing=Path(tmp)/'capture.ffconcat'
        listing.write_text('\n'.join(manifest)+'\n')
        subprocess.run(['ffmpeg','-v','error','-nostdin','-n','-f','concat','-safe','0',
            '-i',str(listing),'-vf','crop=640:360:0:0,scale=out_color_matrix=bt709:out_range=full,'
            'format=yuv444p12,colorspace=iall=bt709:itrc=srgb:irange=pc:all=bt709:range=tv:format=yuv420p',
            '-fps_mode','passthrough','-enc_time_base','1:1000','-video_track_timescale','1000',
            '-c:v','libx264','-crf','14','-preset','fast','-g','36','-bf','2','-color_range','tv',
            '-color_primaries','bt709','-color_trc','bt709','-colorspace','bt709','-an',str(destination/'recording.mp4')],check=True)
    capture.update({'units':'milliseconds since first compositor frame',
                    'columns':['ms','browser revision barcode','optional original PNG filename'],'pictures':oracle})
    (destination/'capture.json').write_text(json.dumps(capture,separators=(',',':'))+'\n')


if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('capture_directory',type=Path)
    parser.add_argument('fixture_directory',type=Path)
    args=parser.parse_args()
    package(args.capture_directory,args.fixture_directory)
