import type { Picture, ViewerBridge, ViewerDocument } from '../client/types.ts';

export function imageData(picture: Picture) {
  const { rgba, width, height } = picture;
  return new ImageData(new Uint8ClampedArray(rgba.buffer as ArrayBuffer, rgba.byteOffset, rgba.byteLength), width, height);
}

export interface PreviewEvents {
  picture(frame: number, playing: boolean): void;
  playback(playing: boolean, startFrame?: number): void;
  seeking?(active: boolean): void;
  error(error: unknown): void;
}

const generations = new WeakMap<ViewerBridge, number>();

/** Coalesces pointer samples and keeps the parked picture available during a skim. */
export class PreviewController {
  private present: (picture: Picture) => void;
  private wait: () => Promise<void>;
  private bridge: ViewerBridge;
  private revision: number;
  private duration: number;
  private events: PreviewEvents;
  private generation = 0;
  private epoch = 0;
  private seeking = false;
  private target?: { frame: number; epoch: number; skim: boolean };
  private skimFrame?: number;
  private parkedFrame?: number;
  private parkedPicture?: Picture;
  private playing = false;
  private disposed = false;

  constructor(bridge: ViewerBridge, doc: ViewerDocument, events: PreviewEvents,
              present: (picture: Picture) => void, wait: () => Promise<void>) {
    this.bridge = bridge; this.events = events; this.present = present; this.wait = wait;
    this.revision = doc.revision; this.duration = doc.snapshot.duration;
  }

  private advance() {
    this.generation = (generations.get(this.bridge) ?? 0) + 1;
    generations.set(this.bridge, this.generation);
    return this.generation;
  }

  private draw(picture: Picture) {
    this.present(picture);
    this.events.picture(picture.frame, this.playing);
  }

  private clampFrame(frame: number) {
    return Math.max(0, Math.min(this.duration - 1, frame));
  }

  private queue(frame: number, skim: boolean) {
    this.target = { frame, skim, epoch: this.epoch };
    void this.drain();
  }

  private invalidateSeeks() {
    this.epoch++;
    this.target = undefined;
    this.skimFrame = undefined;
  }

  private cachedParkedPicture() {
    if (this.parkedPicture?.frame === this.parkedFrame) return this.parkedPicture;
  }

  seek(frame: number, phase: 'scrub' | 'exact') {
    if (this.disposed) return;
    if (this.playing) this.stop();
    if (phase === 'exact' || this.skimFrame !== undefined) this.invalidateSeeks();
    this.parkedFrame = this.clampFrame(frame);
    this.queue(this.parkedFrame, false);
  }

  skim(frame: number) {
    if (this.disposed || this.playing) return;
    const target = this.clampFrame(frame);
    if (target === this.parkedFrame) {
      this.clearSkim();
      return;
    }
    if (target === this.skimFrame) return;
    if (this.skimFrame === undefined) this.invalidateSeeks();
    this.skimFrame = target;
    this.queue(target, true);
  }

  clearSkim() {
    if (this.disposed || this.skimFrame === undefined) return;
    this.invalidateSeeks();
    const parked = this.cachedParkedPicture();
    if (parked) this.draw(parked);
    else if (this.parkedFrame !== undefined) this.queue(this.parkedFrame, false);
  }

  private async drain() {
    if (this.seeking) return;
    this.seeking = true;
    try {
      this.events.seeking?.(true);
      while (this.target && !this.disposed) {
        const target = this.target;
        this.target = undefined;
        const g = this.advance();
        try {
          const picture = await this.bridge.seek(target.frame, g, target.skim);
          if (this.disposed || target.epoch !== this.epoch || picture.revision !== this.revision) continue;
          if (!target.skim) this.parkedPicture = picture;
          this.draw(picture);
        } catch (error) {
          if (!this.disposed && target.epoch === this.epoch) this.events.error(error);
        }
      }
    } finally {
      this.seeking = false;
      if (!this.disposed) this.events.seeking?.(false);
    }
  }

  async play(frame: number, skimFrame = this.skimFrame) {
    if (this.playing || this.disposed) return;
    const start = skimFrame ?? (frame >= this.duration - 1 ? 0 : frame);
    this.invalidateSeeks();
    const g = this.advance();
    this.parkedFrame = start;
    this.playing = true;
    this.events.playback(true, start);
    try {
      await this.bridge.play(start, g);
      while (this.playing && g === this.generation && !this.disposed) {
        const sample = await this.bridge.next(g);
        if (g !== this.generation || this.disposed) break;
        if (sample.picture && sample.picture.revision === this.revision) {
          this.parkedFrame = sample.picture.frame;
          this.parkedPicture = sample.picture;
          this.draw(sample.picture);
          await this.bridge.presented(sample.picture.frame, g);
        } else if (!sample.playing) break;
        else await this.wait();
      }
    } catch (error) {
      if (!this.disposed && g === this.generation) this.events.error(error);
    } finally {
      if (this.generation === g) this.pause();
    }
  }

  private stop() {
    this.invalidateSeeks();
    if (!this.playing) return;
    this.playing = false; this.events.playback(false);
    void this.bridge.pause(this.advance()).catch(error => {
      if (!this.disposed) this.events.error(error);
    });
  }

  pause() {
    if (this.skimFrame !== undefined) {
      this.clearSkim();
      return;
    }
    if (!this.playing) return;
    this.stop();
    // A pause before the first playback picture still lands on the start frame.
    if (this.parkedFrame !== undefined && !this.cachedParkedPicture())
      this.queue(this.parkedFrame, false);
  }

  replace(doc: ViewerDocument, parked?: Picture) {
    if (parked && (parked.revision !== doc.revision || parked.frame >= doc.snapshot.duration))
      throw new Error('Cannot install parked picture from another document');
    this.stop(); this.revision = doc.revision; this.duration = doc.snapshot.duration;
    this.parkedFrame = parked?.frame;
    this.parkedPicture = parked;
    if (parked && !this.disposed) this.draw(parked);
  }

  dispose() {
    this.disposed = true;
    this.stop();
    this.parkedPicture = undefined;
  }
}

export function mountPreview(canvas: HTMLCanvasElement, bridge: ViewerBridge, doc: ViewerDocument, events: PreviewEvents) {
  const context = canvas.getContext('2d', { alpha: true });
  if (!context) throw new Error('Preview canvas unavailable');
  return new PreviewController(bridge, doc, events, picture => {
    if (canvas.width !== picture.width) canvas.width = picture.width;
    if (canvas.height !== picture.height) canvas.height = picture.height;
    context.putImageData(imageData(picture), 0, 0);
    canvas.dataset.frame = String(picture.frame);
    canvas.dataset.revision = String(picture.revision);
  }, () => new Promise(resolve => requestAnimationFrame(() => resolve())));
}
