import { contextBridge, ipcRenderer } from 'electron';
import type { ViewerBridge } from '../client/types.ts';

export function exposeViewer(name = 'n4Viewer') {
  // The parent can unmount its surfaces while the granted project stays open.
  // Hold one missed hint so the next mount reconciles with disk.
  const listeners = new Set<() => void>();
  let pending = false;
  ipcRenderer.on('n4.viewer.inputsChanged', () => {
    pending = listeners.size === 0;
    for (const listener of listeners) listener();
  });
  const bridge: ViewerBridge = {
    onProjectChange(listener) {
      listeners.add(listener);
      if (pending) {
        pending = false;
        listener();
      }
      return () => { listeners.delete(listener); };
    },
    inspect: () => ipcRenderer.invoke('n4.viewer.inspect'),
    reopen: frame => ipcRenderer.invoke('n4.viewer.reopen', ...(frame === undefined ? [] : [frame])),
    seek: (f, g, previewOnly) => ipcRenderer.invoke(
      'n4.viewer.seek', f, g, ...(previewOnly === undefined ? [] : [previewOnly]),
    ),
    thumbnail: (frame, generation, placement) => ipcRenderer.invoke(
      'n4.viewer.thumbnail', frame, generation, ...(placement === undefined ? [] : [placement]),
    ),
    play: (f, g) => ipcRenderer.invoke('n4.viewer.play', f, g),
    next: g => ipcRenderer.invoke('n4.viewer.next', g),
    pause: g => ipcRenderer.invoke('n4.viewer.pause', g),
    presented: (f, g) => ipcRenderer.invoke('n4.viewer.presented', f, g),
    editor: command => ipcRenderer.invoke('n4.viewer.editor', command),
    hud: (revision, frame) => ipcRenderer.invoke('n4.viewer.hud', revision, frame),
  };
  contextBridge.exposeInMainWorld(name, bridge);
}
