import { contextBridge, ipcRenderer } from 'electron';
import type { ExportDialogBridge } from './types.ts';

const bridge: ExportDialogBridge = {
  inspect: () => ipcRenderer.invoke('n4.export.inspect'),
  startExport: revision => ipcRenderer.invoke('n4.export.start', revision),
  exportStatus: () => ipcRenderer.invoke('n4.export.status'),
  cancelExport: () => ipcRenderer.invoke('n4.export.cancel'),
  requestClose: () => ipcRenderer.invoke('n4.export.close'),
};

contextBridge.exposeInMainWorld('n4Export', bridge);
