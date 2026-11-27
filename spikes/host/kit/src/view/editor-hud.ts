import type { ViewerBridge, ViewerDocument } from '../client/types.ts';
import { imageData } from './preview.ts';

/** Presents the latest Rust-rendered selection tile over the matching film picture. */
export function mountEditorHud(canvas: HTMLCanvasElement, bridge: ViewerBridge, onError: (error: unknown) => void) {
  const context = canvas.getContext('2d');
  if (!context || !bridge.hud) throw new Error('Editor HUD unavailable');
  const raster = context;
  let version = 0;
  let busy = false;
  let picture: { revision: number; frame: number } | undefined;
  let painted: [number, number, number, number] | undefined;
  let pending: { version: number; revision: number; frame: number; hudVersion: number } | undefined;

  async function present() {
    if (busy || !pending) return;
    busy = true;
    try {
      while (pending) {
        const target = pending;
        pending = undefined;
        try {
          const reply = await bridge.hud!(target.revision, target.frame);
          if (target.version !== version || reply.hudVersion < target.hudVersion) continue;
          const { tile } = reply;
          erase();
          if (tile && tile.revision === target.revision && tile.frame === target.frame) {
            raster.putImageData(imageData(tile), tile.x, tile.y);
            painted = [tile.x, tile.y, tile.width, tile.height];
            canvas.dataset.frame = String(tile.frame);
          }
        } catch (error) {
          if (target.version === version) onError(error);
        }
      }
    } finally {
      busy = false;
    }
  }

  function erase() {
    if (painted) {
      raster.clearRect(...painted);
      painted = undefined;
      delete canvas.dataset.frame;
    }
  }

  return {
    clear() {
      version++;
      pending = undefined;
      picture = undefined;
      erase();
    },
    draw(doc: ViewerDocument, frame: number, hudVersion: number) {
      if (picture?.revision !== doc.revision || picture.frame !== frame) erase();
      if (canvas.width !== doc.size[0]) canvas.width = doc.size[0];
      if (canvas.height !== doc.size[1]) canvas.height = doc.size[1];
      picture = { revision: doc.revision, frame };
      pending = { version: ++version, ...picture, hudVersion };
      void present();
    },
  };
}
