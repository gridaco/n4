// Consumer fixture built outside the checkout using only public kit entry points.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { build } from 'esbuild';

const host = fileURLToPath(new URL('../', import.meta.url));
const temp = mkdtempSync(join(tmpdir(), 'n4-parent-'));
const require = createRequire(import.meta.url);
async function bundle(name, contents, platform) {
  await build({ stdin: {contents, resolveDir: host}, bundle: true, platform,
    format: platform === 'browser' ? 'iife' : 'cjs', external: ['electron'], outfile: join(temp, name) });
}
try {
  await bundle('preload.cjs', "import {exposeViewer} from '@n4/host-kit/electron-preload'; exposeViewer('film');", 'node');
  await bundle('renderer.js', `
    import {mountViewer} from '@n4/host-kit/viewer';
    async function run() {
      // A product can replace its document tab while retaining its window grant.
      for (const frame of [24, 42]) {
        const preview = document.createElement('canvas'), timeline = document.createElement('div');
        timeline.style.height = '260px';
        document.body.replaceChildren(preview, timeline);
        let resolvePicture, rejectPicture;
        const drawn = new Promise((res, rej) => { resolvePicture = res; rejectPicture = rej; });
        const viewer = await mountViewer(window.film, {preview, timeline, frame,
          onPicture: resolvePicture, onError: rejectPicture});
        if (await drawn !== frame) throw Error('Parent received the wrong picture');
        viewer.dispose();
        if (timeline.hasChildNodes()) throw Error('Timeline controls survived disposal');
      }
      document.title = 'passed';
    }
    run().catch(e => {document.title = 'failed ' + e.message;});
  `, 'browser');
  await bundle('parent.cjs', String.raw`
    import {app,BrowserWindow} from 'electron';
    import {join} from 'node:path';
    import {pathToFileURL} from 'node:url';
    import {bindViewer} from '@n4/host-kit/electron-main';
    app.whenReady().then(async () => {
      const win = new BrowserWindow({show:false,webPreferences:{backgroundThrottling:false,
        preload:join(__dirname,'preload.cjs'), sandbox:true, contextIsolation:true, nodeIntegration:false}});
      const rendererURL = pathToFileURL(join(__dirname,'view.html')).href;
      const dispose = bindViewer(win.webContents, {rendererURL, project:process.argv[2], executable:process.argv[3]});
      win.on('page-title-updated', async (_, title) => {
        if (title === 'passed' || title.startsWith('failed ')) {
          if (title !== 'passed') console.error(title);
          try { await dispose(); }
          catch (error) { console.error(error); app.exit(1); return; }
          win.destroy(); app.exit(title === 'passed' ? 0 : 1);
        }
      });
      await win.loadURL(rendererURL);
    }).catch(e => {console.error(e); app.exit(1);});
  `, 'node');
  copyFileSync(require.resolve('@n4/host-kit/timeline.css'), join(temp, 'timeline.css'));
  writeFileSync(join(temp, 'view.html'), '<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'self\'; style-src \'self\'"><link rel="stylesheet" href="timeline.css"><body><script src="renderer.js"></script>');
  execFileSync(require('electron'), [join(temp,'parent.cjs'),
    resolve(host,'../examples/signal-atlas'), resolve(host,'../engine/target/debug/n4-host')],
    {cwd:temp, timeout:90000, stdio:'inherit'});
  console.log('Independent parent received native frames across kit mount, dispose and remount.');
} finally {
  rmSync(temp,{recursive:true,force:true});
}
