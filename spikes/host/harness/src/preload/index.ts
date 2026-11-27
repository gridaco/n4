import { contextBridge, ipcRenderer } from 'electron';
import { exposeViewer } from '@n4/host-kit/electron-preload';
exposeViewer();
contextBridge.exposeInMainWorld('openN4Export', () => ipcRenderer.invoke('n4.viewer.openExport'));
