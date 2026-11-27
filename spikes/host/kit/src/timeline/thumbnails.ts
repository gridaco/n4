import type { Placement, TimelineLayout } from './index.ts';

interface ThumbnailTile {
  placement: Placement;
  start: number;
  end: number;
  frame: number;
  key: string;
}

const targetWidth = 76; // Logical pixels per filmstrip tile.

export function planThumbnails(layout: TimelineLayout) {
  const tiles = new Map<string, ThumbnailTile[]>();
  const wanted = new Map<string, ThumbnailTile>();
  const [first, last] = layout.visible;
  const { pixelsPerFrame } = layout;
  const stride = Math.max(1, 2 ** Math.round(Math.log2(targetWidth / pixelsPerFrame)));

  for (const row of layout.rows) {
    for (const { placement } of row.clips) {
      if (!placement.track.startsWith('V')) continue;
      const [lo, hi] = placement.project;
      if ((hi - lo) * pixelsPerFrame <= 1) continue; // Only the clip divider fits.
      const visible: ThumbnailTile[] = [];
      // Skip whole cells from the placement origin. A partial cell keeps its full picture.
      const offset = Math.max(0, Math.floor((first - lo) / stride)) * stride;
      for (let start = lo + offset; start < Math.min(last, hi); start += stride) {
        const end = Math.min(start + stride, hi);
        const frame = start + Math.floor((end - start - 1) / 2);
        const key = `${placement.path}\0${frame}`;
        visible.push({ placement, start, end, frame, key });
      }
      if (visible.length) {
        tiles.set(placement.path, visible);
        // Fetch a poster for every clip before filling in the longer filmstrips.
        wanted.set(visible[0].key, visible[0]);
      }
    }
  }
  for (const visible of tiles.values()) {
    for (let i = 1; i < visible.length; i++) wanted.set(visible[i].key, visible[i]);
  }
  return { tiles, wanted };
}
