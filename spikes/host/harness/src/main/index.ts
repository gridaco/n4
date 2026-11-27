import { app, BrowserWindow, dialog } from 'electron';
import { resolve, join, dirname, extname } from 'node:path';
import { existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { bindViewer } from '@n4/host-kit/electron-main';
import { ExportWindow } from './export-window.ts';

app.setName('N4 Viewer');
app.on('window-all-closed', () => app.quit());
void app.whenReady().then(async () => {
  const args = process.argv.slice(process.defaultApp ? 2 : 1);
  let project = args.find(a => !a.startsWith('--'));
  if (!project) {
    const choice = await dialog.showOpenDialog({ title: 'Open an N4 project', properties: ['openDirectory'] });
    if (choice.canceled) { app.quit(); return; }
    project = choice.filePaths[0];
  }
  const executable = process.env.N4_HOST_BIN ?? (app.isPackaged ?
    join(process.resourcesPath, 'native', 'n4-host') : resolve(__dirname, '../../../engine/target/debug/n4-host'));
  const insetTitleBar = process.platform === 'darwin';
  const url = pathToFileURL(join(__dirname, 'index.html'));
  const start = args.find(a => /^--frame=\d+$/.test(a));
  if (start) url.searchParams.set('frame', start.slice(8));
  if (insetTitleBar) url.searchParams.set('insetTitleBar', '1');
  const win = new BrowserWindow({
    title: 'N4 Viewer', width: 1160, height: 820, minWidth: 600, minHeight: 460,
    backgroundColor: '#151618',
    ...(insetTitleBar && { titleBarStyle: 'hiddenInset', trafficLightPosition: { x: 14, y: 14 } }),
    webPreferences: { preload: join(__dirname, 'preload.cjs'), contextIsolation: true, sandbox: true, nodeIntegration: false },
  });
  const contents = win.webContents;
  contents.setWindowOpenHandler(() => ({ action: 'deny' }));
  contents.on('will-navigate', (event, destination) => { if (destination !== url.href) event.preventDefault(); });
  contents.session.setPermissionRequestHandler((_wc, _permission, answer) => answer(false));
  if (app.isPackaged) {
    // Native media commands resolve to the tools shipped with this harness.
    process.env.PATH = [dirname(executable), process.env.PATH].filter(Boolean).join(':');
  }
  const exportWindow = new ExportWindow();
  contents.on('did-navigate', () => exportWindow.dispose());
  contents.on('render-process-gone', () => exportWindow.dispose());
  const projectPath = resolve(project);
  const editor = extname(projectPath) === '.proj' || existsSync(join(projectPath, 'project.proj'));
  const viewer = bindViewer(contents, {
    executable,
    project: projectPath,
    editor,
    rendererURL: url.href,
    chooseExportPath: name => exportWindow.choosePath(name),
  });
  let closing = false;
  let canClose = false;
  contents.ipc.handle('n4.viewer.openExport', event => {
    if (closing || !event.senderFrame || event.senderFrame !== contents.mainFrame ||
        event.senderFrame.url !== url.href) throw new Error('Unknown export launcher');
    return exportWindow.open(win, viewer);
  });
  win.on('close', event => {
    if (canClose) return;
    event.preventDefault();
    if (closing) return;
    closing = true;
    exportWindow.dispose();
    void viewer({ closingWindow: true }).catch(console.error).finally(() => {
      canClose = true;
      if (!win.isDestroyed()) win.close();
    });
  });
  await win.loadURL(url.href);
}).catch(error => { console.error(error); app.exit(1); });
