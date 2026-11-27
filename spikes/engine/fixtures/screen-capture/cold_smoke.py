"""Run staged binaries twice with empty application caches and minimal child environments."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ENGINE = Path(__file__).resolve().parents[2]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def trial(cli, device, root):
    profile=root/'profile'
    for name in ['config','cache','data','state','tmp']:
        (profile/name).mkdir(parents=True,exist_ok=True)
    bindir=root/'bin'
    bindir.mkdir()
    n4=Path(shutil.copy2(cli,bindir/'n4'))
    for name in ['ffmpeg','ffprobe']:
        src=shutil.which(name)
        if not src: raise RuntimeError(name+' missing')
        (bindir/name).symlink_to(src)
    # Only tested child processes see the isolated environment. The parent shell is unchanged.
    env={'PATH':str(bindir)+':/usr/bin:/bin','HOME':str(profile),
         'XDG_CACHE_HOME':str(profile/'cache'),'XDG_CONFIG_HOME':str(profile/'config'),
         'XDG_DATA_HOME':str(profile/'data'),'XDG_STATE_HOME':str(profile/'state'),
         'TMPDIR':str(profile/'tmp'),'LC_ALL':'C'}
    projectdir=root/'fresh project ü'
    projectdir.mkdir()
    shared=projectdir/'fonts'
    shared.mkdir()
    shutil.copy2(ENGINE/'fixtures/fonts/N4GridSans.ttf',shared)
    work=projectdir/'av-cut'
    work.mkdir()
    assets=['scope.mkv','lead.wav','cues.wav','edit.json']
    for f in assets: shutil.copy2(ENGINE/'fixtures/av-cut'/f,work)
    def run(cmd):
        return subprocess.check_output(cmd,cwd=work,env=env)
    run([n4,'render','edit.json','--frames','150,149,150','--out','inspection'])
    hashes=list(map(digest,sorted((work/'inspection').glob('*.png'))))
    assert len(hashes)==3 and hashes[0]==hashes[2] and hashes[0]!=hashes[1]
    exported=json.loads(run([n4,'export','edit.json','--out','movie.mp4']))
    probe=json.loads(run(['ffprobe','-v','error','-show_entries','stream=codec_name,duration,nb_frames',
                          '-of','json','movie.mp4']))['streams']
    assert exported['frames']==300 and exported['audio_sample_frames']==480000
    assert probe[0]['nb_frames']=='300' and probe[1]['codec_name']=='aac'
    values={'native_binary_sha256':digest(n4),'fixture_assets_sha256':{n:digest(work/n) for n in assets},
        'font_sha256':digest(shared/'N4GridSans.ttf'),'out_of_order_pngs_sha256':hashes,
        'complete_export':exported,'video_audio_streams':probe}
    if device:
        tool=Path(shutil.copy2(device,bindir/'n4-device'))
        run([tool,'edit.json','--seconds','1','--report','physical.json'])
        values['native_device']=json.loads((work/'physical.json').read_text())
        values['device_binary_sha256']=digest(tool)
    return values


if __name__=='__main__':
    p=argparse.ArgumentParser()
    p.add_argument('cli',type=Path)
    p.add_argument('--device',type=Path,help='also open a one-second native window on the second clean run')
    args=p.parse_args()
    with tempfile.TemporaryDirectory() as staging:
        root=Path(staging)
        runs=[trial(args.cli.resolve(), args.device.resolve() if args.device and i==1 else None,
                    root/str(i)) for i in range(2)]
    assert runs[0]['complete_export']==runs[1]['complete_export']
    assert runs[0]['out_of_order_pngs_sha256']==runs[1]['out_of_order_pngs_sha256']
    print(json.dumps({'scope':'two new process profiles and empty rebuildable application caches; same OS account and system libraries',
                      'environment_keys':['HOME','PATH','TMPDIR','LC_ALL','XDG_CACHE_HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_STATE_HOME'],
                      'repeat_matches':True,'runs':runs},indent=2))
