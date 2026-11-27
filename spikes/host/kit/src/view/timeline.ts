import { bodyTop, headerHeight, rowHeight } from '../timeline/index.ts';
import type { Placement, TimelineController, TimelineLayout } from '../timeline/index.ts';
import type { TimelineChange } from '../client/types.ts';
import { planThumbnails } from '../timeline/thumbnails.ts';
import { timelineTicks } from '../timeline/time.ts';
import { createTimelineControls } from './timeline-controls.ts';
import { ThumbnailCache } from './thumbnail-cache.ts';

export interface TimelineMedia {
  peaks?: readonly number[];
  thumbnail?: (placement: Placement, frame: number) => Promise<ImageData | null>;
}

const thumbnailCacheLimit = 64;

export function mountTimeline(
  mount: HTMLElement,
  controller: TimelineController,
  initial: TimelineMedia = {},
) {
  const controls = createTimelineControls(controller);
  const { root, plot, canvas, handle } = controls;
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('The timeline requires a 2D canvas');
  const g = ctx;
  mount.append(root);
  let media = initial;
  let active = true;
  let raf = 0;
  let drag: { pointer: number; target: HTMLElement; offset: number } | null = null;
  let layout: TimelineLayout | undefined;
  let selection = controller.selection;
  let thumbnailLayout: TimelineLayout | undefined;
  let thumbnails = planThumbnails(controller.layout());
  let pending = thumbnails.wanted.values();
  let busy = false;
  let refreshing = false;
  let playing = false;
  let seeking = false;
  const bitmaps = new ThumbnailCache<HTMLCanvasElement>(thumbnailCacheLimit);
  const abort = new AbortController();
  const signal = abort.signal;

  function invalidate() {
    if (active && !raf) raf = requestAnimationFrame(draw);
  }

  function updateThumbnails(next: TimelineLayout) {
    if (thumbnailLayout === next) return;
    thumbnailLayout = next;
    thumbnails = planThumbnails(next);
    pending = thumbnails.wanted.values();
    bitmaps.trim(thumbnails.wanted);
  }

  async function sample() {
    if (busy || !active || refreshing || playing || seeking || controller.dragging || !media.thumbnail) return;
    updateThumbnails(controller.layout());
    let request = pending.next();
    while (!request.done && bitmaps.has(request.value.key)) request = pending.next();
    if (request.done) return;
    const { key, placement, frame, start, end } = request.value;
    const entry = bitmaps.begin(key, placement.path, frame, start, end);
    busy = true;
    try {
      const data = await media.thumbnail(placement, frame);
      if (!active || !bitmaps.accepts(entry)) return;
      if (data) {
        const tile = document.createElement('canvas');
        tile.width = data.width;
        tile.height = data.height;
        tile.getContext('2d')!.putImageData(data, 0, 0);
        bitmaps.complete(entry, tile);
      } else bitmaps.complete(entry, null);
    } catch {
      if (active) bitmaps.complete(entry, null);
    } finally {
      busy = false;
      bitmaps.trim(thumbnails.wanted);
      invalidate();
      // Each completed request can start the next visible sample even if the
      // window is occluded and Chromium has suspended paint callbacks.
      void sample();
    }
  }

  function text(s: string, x: number, y: number, color: string, maxWidth?: number) {
    if (maxWidth !== undefined && g.measureText(s).width > maxWidth) {
      if (maxWidth < g.measureText('…').width) return;
      const chars = Array.from(s);
      let first = 0;
      let last = chars.length;
      while (first < last) {
        const count = Math.ceil((first + last) / 2);
        const candidate = chars.slice(0, count).join('') + '…';
        if (g.measureText(candidate).width <= maxWidth) first = count;
        else last = count - 1;
      }
      s = chars.slice(0, first).join('') + '…';
    }
    g.fillStyle = color;
    g.fillText(s, x, y);
  }

  function draw() {
    raf = 0;
    if (!active) return;
    const l = controller.layout();
    updateThumbnails(l);
    const { width: w, height: h, gutter } = l;
    const plotWidth = w - gutter;
    const plotHeight = Math.max(1, h - headerHeight);
    const ratio = window.devicePixelRatio || 1;
    const bw = Math.max(1, Math.round(plotWidth * ratio));
    const bh = Math.max(1, Math.round(plotHeight * ratio));
    if (canvas.width !== bw) canvas.width = bw;
    if (canvas.height !== bh) canvas.height = bh;
    // Keep hit testing in mount coordinates. The bitmap holds only the plot.
    const scaleX = bw / plotWidth;
    const scaleY = bh / plotHeight;
    g.setTransform(scaleX, 0, 0, scaleY, -gutter * scaleX, -headerHeight * scaleY);
    g.fillStyle = '#18181b';
    g.fillRect(gutter, headerHeight, plotWidth, plotHeight);
    g.fillStyle = '#222225';
    g.fillRect(gutter, headerHeight, plotWidth, bodyTop - headerHeight);
    g.textBaseline = 'middle';
    g.font = '12px system-ui, sans-serif';
    for (const tick of timelineTicks(l.visible, l.scope[0], controller.fps, plotWidth)) {
      const x = controller.frameToX(tick.frame);
      g.fillStyle = '#52525b';
      g.fillRect(x, bodyTop - 7, 1, 7);
      text(tick.label, x + 4, bodyTop - 12, '#b4b4bc');
    }
    // Round shared tile edges in bitmap space so fractional positions cannot leave seams.
    const snapX = (x: number) => gutter + Math.round((x - gutter) * scaleX) / scaleX;
    g.save();
    g.beginPath();
    g.rect(gutter, bodyTop, plotWidth, Math.max(0, h - bodyTop));
    g.clip();
    for (const row of l.rows) {
      const y = row.top,
        mid = y + row.height / 2;
      const selected = row.selected || row.id === controller.selection?.id;
      g.fillStyle = '#27272a';
      g.fillRect(gutter, y + row.height - 1, plotWidth, 1);
      g.save();
      g.beginPath();
      g.rect(gutter, y, plotWidth, row.height);
      g.clip();
      for (const clip of row.clips) {
        const { placement, x } = clip;
        const cw = Math.max(1, clip.right - x - 1);
        const top = y + 3;
        const height = row.height - 6;
        const isPicture = placement.track.startsWith('V');
        const isSelected = clip.selected || placement.path === controller.selection?.id;
        if (placement.enabled === false) g.globalAlpha = 0.42;
        g.fillStyle = isSelected ? '#45454c' : '#303035';
        g.fillRect(x, top, cw, height);
        if (isPicture) {
          const labelHeight = 18;
          const posterHeight = height - labelHeight;
          const clipTiles = thumbnails.tiles.get(placement.path);
          if (clipTiles) {
            for (const sample of clipTiles) {
              const tile = bitmaps.forDisplay(sample.key, placement.path, sample.frame, sample.start, sample.end);
              if (!tile) continue;
              const left = snapX(controller.frameToX(sample.start));
              const divider = sample.end === placement.project[1] ? 1 : 0;
              const width = snapX(controller.frameToX(sample.end) - divider) - left;
              if (width <= 0) continue;
              const scale = Math.max(width / tile.width, posterHeight / tile.height);
              const sourceWidth = width / scale;
              const sourceHeight = posterHeight / scale;
              g.drawImage(tile, (tile.width - sourceWidth) / 2, (tile.height - sourceHeight) / 2,
                sourceWidth, sourceHeight, left, top, width, posterHeight);
            }
          }
          g.fillStyle = isSelected ? '#45454c' : '#27272a';
          g.fillRect(x, top + posterHeight, cw, labelHeight);
          text(placement.name, x + 6, top + posterHeight + labelHeight / 2, '#ededf0', cw - 12);
        } else {
          text(placement.name, x + 6, mid, '#ededf0', cw - 12);
        }
        g.globalAlpha = 1;
        if (isSelected) {
          g.fillStyle = '#aad2f5';
          g.fillRect(x, top, cw, 2);
        }
      }
      if (row.range) {
        const x = controller.frameToX(row.range[0]),
          right = controller.frameToX(row.range[1]);
        g.fillStyle = selected ? '#aad2f5' : '#3f3f46';
        g.fillRect(x, mid - 3, right - x, 6);
      }
      for (const k of row.keys) {
        g.fillStyle = selected && controller.selection?.frame === k.frame ? '#aad2f5' : '#a1a1aa';
        g.beginPath();
        g.moveTo(k.x, mid - 5);
        g.lineTo(k.x + 5, mid);
        g.lineTo(k.x, mid + 5);
        g.lineTo(k.x - 5, mid);
        g.closePath();
        g.fill();
      }
      if (row.kind === 'mix' && media.peaks?.length) {
        const peaks = media.peaks;
        g.fillStyle = '#a1a1aa';
        // Include every bucket spanned by a pixel so a short transient survives zoom-out.
        for (let x = gutter; x < w; x += 2) {
          const a = Math.floor((controller.frameAt(x) * peaks.length) / controller.duration);
          const b = Math.min(
            peaks.length,
            Math.ceil(((controller.frameAt(x + 2) + 1) * peaks.length) / controller.duration),
          );
          let amp = 0;
          for (let i = a; i < b; ++i) amp = Math.max(amp, Math.abs(peaks[i]!));
          const height = Math.min(1, amp) * (row.height - 5);
          g.fillRect(x, mid - height / 2, 1.5, height);
        }
      }
      g.restore();
    }
    g.restore();
    const paths = [...thumbnails.wanted.keys()];
    // The presentation benchmark starts after the visible samples settle.
    const readyText = String(paths.some((id) => bitmaps.get(id)) && paths.every((id) => bitmaps.get(id) !== undefined));
    if (canvas.dataset.thumbnailsReady !== readyText) canvas.dataset.thumbnailsReady = readyText;
    void sample();
  }

  const point = (e: MouseEvent) => {
    const b = root.getBoundingClientRect();
    return { x: e.clientX - b.left, y: e.clientY - b.top };
  };
  function skim(e: PointerEvent) {
    if (e.pointerType === 'touch' || e.buttons !== 0 || drag) return;
    controller.skimAt(point(e).x);
  }
  plot.addEventListener('pointermove', skim, { signal });
  plot.addEventListener('pointerleave', () => controller.clearSkim(), { signal });
  window.addEventListener('blur', () => controller.clearSkim(), { signal });
  function capture(target: HTMLElement, pointer: number, offset: number) {
    drag = { pointer, target, offset };
    target.setPointerCapture(pointer);
  }
  canvas.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || drag) return;
    const { x, y } = point(e);
    canvas.focus({ preventScroll: true });
    controller.pointerDown(x, y);
    if (controller.dragging) capture(canvas, e.pointerId, 0);
  }, { signal });
  handle.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || drag) return;
    e.preventDefault();
    canvas.focus({ preventScroll: true });
    const { x } = point(e);
    // Preserve where the pointer grabbed the handle. Aim inside the current
    // frame so rounding at a fractional frame boundary cannot change it.
    const center = controller.frameToX(controller.cursor + 0.5);
    controller.beginScrub(center);
    capture(handle, e.pointerId, center - x);
  }, { signal });
  // PointerEvent.detail is browser dependent; use the native double-click event to enter clips.
  canvas.addEventListener('dblclick', (e) => {
    const { x, y } = point(e);
    if (controller.hitTest(x, y)?.kind === 'clip') controller.pointerDown(x, y, 2);
  }, { signal });
  root.addEventListener('pointermove', (e) => {
    if (drag && e.pointerId === drag.pointer) controller.pointerMove(point(e).x + drag.offset);
  }, { signal });
  root.addEventListener('pointerup', (e) => {
    if (!drag || e.pointerId !== drag.pointer) return;
    const { offset } = drag;
    drag = null;
    controller.pointerUp(point(e).x + offset);
  }, { signal });
  function cancel(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.pointer) return;
    drag = null;
    controller.pointerUp();
  }
  root.addEventListener('pointercancel', cancel, { signal });
  root.addEventListener('lostpointercapture', cancel, { signal });
  root.addEventListener('keydown', (e) => {
    if (e.target instanceof HTMLButtonElement && (e.key === 'Enter' || e.key === ' ')) return;
    if (controller.keyboard(e.key)) e.preventDefault();
  }, { signal });
  root.addEventListener('wheel', (e) => {
    e.preventDefault();
    const unit = e.deltaMode === 1 ? rowHeight : e.deltaMode === 2 ? root.clientHeight : 1;
    if (e.ctrlKey || e.metaKey)
      controller.zoomAt(
        Math.exp(Math.min(4, Math.max(-4, (e.deltaY * unit) / 160))),
        point(e).x,
      );
    else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY))
      controller.pan((e.deltaX || e.deltaY) * unit);
    else controller.scrollRows(e.deltaY * unit);
  }, { passive: false, signal });

  function update() {
    const next = controller.layout();
    const layoutChanged = next !== layout;
    if (layoutChanged) {
      controls.layout(next);
      layout = next;
    }
    if (layoutChanged || selection !== controller.selection) {
      controls.selection();
      selection = controller.selection;
      invalidate();
    }
    controls.cursor(next);
    // The controller publishes its state before dispatching a seek intent.
    if (!layoutChanged && !controller.dragging) queueMicrotask(sample);
  }
  const unsub = controller.subscribe(update);
  const resize = () => {
    const b = root.getBoundingClientRect();
    controller.resize(b.width, b.height);
    invalidate();
  };
  const observer = new ResizeObserver(resize);
  observer.observe(root);
  window.addEventListener('resize', resize, { signal });
  resize();
  function setMedia(next: TimelineMedia, change?: TimelineChange) {
    media = next;
    thumbnailLayout = undefined;
    bitmaps.accept(change, (path, frame) => controller.containsSample(path, frame));
    controller.setAudioMix(!!next.peaks?.length);
    controls.flashChanges(change);
    // Publish the first layout even if the window was mounted while occluded.
    cancelAnimationFrame(raf);
    draw();
  }
  setMedia(initial);
  return {
    setMedia,
    setRefreshing(value: boolean) {
      refreshing = value;
      if (!value) queueMicrotask(sample);
    },
    setPlaying(value: boolean) {
      playing = value;
      if (!value) queueMicrotask(sample);
    },
    setSeeking(value: boolean) {
      seeking = value;
      if (!value) queueMicrotask(sample);
    },
    dispose() {
      active = false;
      observer.disconnect();
      abort.abort();
      unsub();
      controller.clearSkim();
      if (drag?.target.hasPointerCapture(drag.pointer))
        drag.target.releasePointerCapture(drag.pointer);
      controller.cancelDrag();
      cancelAnimationFrame(raf);
      bitmaps.clear();
      drag = null;
      root.remove();
    },
  };
}
