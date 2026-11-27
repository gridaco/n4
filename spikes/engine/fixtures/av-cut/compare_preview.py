"""Compare all full-quality inspection PNGs to the RGBA digest from an n4 export."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

from PIL import Image


def check_preview(cli, project, report):
    export = json.loads(report.read_text())
    count = export['frames']
    expected = export['rgba_sha256']
    if not count > 0:
        raise ValueError('export report has no frames')
    with tempfile.TemporaryDirectory() as temp:
        frames = Path(temp)/'inspection'
        subprocess.run([str(cli),'render',str(project),'--frames',','.join(map(str,range(count))),
                        '--out',str(frames)], check=True, stdout=subprocess.DEVNULL)
        digest = hashlib.sha256()
        for i in range(count):
            png = frames/f'{i:03}-f{i:04}.png'
            with Image.open(png) as image:
                assert image.mode == 'RGBA'
                digest.update(image.tobytes())
        assert digest.hexdigest()==expected, (digest.hexdigest(),expected)
        return {'inspection_frames':count,'matches_encoder_input':True,'rgba_sha256':expected}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('cli', type=Path)
    parser.add_argument('project', type=Path)
    parser.add_argument('report', type=Path)
    args = parser.parse_args()
    print(json.dumps(check_preview(args.cli.resolve(), args.project, args.report), indent=2))
