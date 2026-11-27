import { BrowserWindow, dialog } from 'electron';
import type { bindViewer } from '@n4/host-kit/electron-main';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as wait } from 'node:timers/promises';

export class ExportWindow {
  private child?: BrowserWindow;
  private closing = false;

  async open(parent: BrowserWindow, viewer: ReturnType<typeof bindViewer>) {
    if (this.child && !this.child.isDestroyed()) {
      this.child.focus();
      return;
    }
    const window = new BrowserWindow({
      parent, modal: true, show: false, title: 'Export Video',
      width: 480, height: 330, useContentSize: true,
      resizable: false, minimizable: false, maximizable: false,
      backgroundColor: '#18181b',
      webPreferences: {
        preload: join(__dirname, 'export-preload.cjs'),
        contextIsolation: true,
        sandbox: true,
        nodeIntegration: false,
      },
    });
    this.child = window;
    this.closing = false;
    const url = pathToFileURL(join(__dirname, 'export.html')).href;
    const contents = window.webContents;
    contents.setWindowOpenHandler(() => ({ action: 'deny' }));
    contents.on('will-navigate', (event, destination) => {
      if (destination !== url) event.preventDefault();
    });
    function handle(
      channel: string,
      arity: number,
      action: (...args: number[]) => unknown,
    ) {
      contents.ipc.handle('n4.export.' + channel, (event, ...args: unknown[]) => {
        if (!event.senderFrame || event.senderFrame !== contents.mainFrame ||
            event.senderFrame.url !== url || args.length !== arity ||
            args.some(value => !Number.isSafeInteger(value) || Number(value) < 0)) {
          throw new Error('Invalid export window request');
        }
        return action(...args.map(Number));
      });
    }
    handle('inspect', 0, async () => {
      const { revision, size, snapshot } = await viewer.inspectForExport();
      return { revision, size, name: snapshot.name, duration: snapshot.duration, fps: snapshot.fps };
    });
    handle('start', 1, revision => {
      if (this.closing) throw new Error('Export window is closing');
      return viewer.startExport(revision!);
    });
    handle('status', 0, () => viewer.exportStatus());
    handle('cancel', 0, () => viewer.cancelExport());
    handle('close', 0, () => { setImmediate(() => { if (!window.isDestroyed()) window.close(); }); });
    let readyToClose = false;
    window.on('close', event => {
      if (readyToClose) return;
      event.preventDefault();
      if (this.closing) return;
      this.closing = true;
      void (async () => {
        const status = await viewer.cancelExport();
        if (status.state === 'running' || status.state === 'cancelling') {
          while (['running', 'cancelling'].includes(viewer.exportStatus().state)) await wait(50);
        }
      })().catch(console.error).finally(() => {
        readyToClose = true;
        if (!window.isDestroyed()) window.close();
      });
    });
    window.once('closed', () => {
      if (this.child === window) this.child = undefined;
    });
    window.once('ready-to-show', () => window.show());
    try {
      await window.loadURL(url);
    } catch (error) {
      if (window.isDestroyed()) return;
      window.destroy();
      throw error;
    }
  }

  async choosePath(suggestedName: string) {
    const owner = this.child;
    if (!owner || owner.isDestroyed() || this.closing) return null;
    const result = await dialog.showSaveDialog(owner, {
      title: 'Export video',
      defaultPath: suggestedName,
      buttonLabel: 'Export',
      filters: [{ name: 'MP4 video', extensions: ['mp4'] }],
      message: 'Choose a new filename.',
      properties: ['createDirectory'],
    });
    return result.canceled || owner !== this.child || this.closing ? null : result.filePath ?? null;
  }

  dispose() {
    this.child?.destroy();
  }
}
