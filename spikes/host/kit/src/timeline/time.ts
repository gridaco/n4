type FrameRate = readonly [numerator: number, denominator: number];

function formatMilliseconds(milliseconds: number, precision: number) {
  const seconds = Math.floor(milliseconds / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const second = String(seconds % 60).padStart(2, '0');
  const clock = hours > 0 ? hours + ':' + String(minutes % 60).padStart(2, '0') : String(minutes);
  const fraction = String(milliseconds % 1000).padStart(3, '0').slice(0, precision);
  return clock + ':' + second + (fraction ? '.' + fraction : '');
}

/** Elapsed playback time of an integer project frame on the project's rational clock. */
export function formatElapsedTime(frame: number, fps: FrameRate) {
  return formatMilliseconds(Math.round(frame * fps[1] * 1000 / fps[0]), 3);
}

const intervals = [
  5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000,
  120_000, 300_000, 600_000, 900_000, 1_800_000, 3_600_000,
];

/** Time ticks are scope-relative. A fractional frame positions a tick between frame boundaries. */
export function timelineTicks(
  [start, end]: readonly [number, number],
  origin: number,
  fps: FrameRate,
  width: number,
): { frame: number; label: string }[] {
  if (width <= 0 || end <= start) return [];
  const desiredFrameSpacing = 84 * (end - start) / width;
  const ticks: { frame: number; label: string }[] = [];
  if (desiredFrameSpacing <= 1) {
    for (let frame = Math.ceil(start); frame < end; frame++) {
      ticks.push({ frame, label: formatElapsedTime(frame - origin, fps) });
    }
    return ticks;
  }
  const millisecondsPerFrame = fps[1] * 1000 / fps[0];
  const desiredTimeSpacing = desiredFrameSpacing * millisecondsPerFrame;
  const hour = 3_600_000;
  const step = intervals.find(ms => ms >= desiredTimeSpacing)
    ?? Math.ceil(desiredTimeSpacing / hour) * hour;
  const precision = step % 1000 === 0 ? 0 : step % 100 === 0 ? 1 : step % 10 === 0 ? 2 : 3;
  const first = Math.ceil((start - origin) * millisecondsPerFrame / step) * step;
  for (let ms = first; ms < (end - origin) * millisecondsPerFrame; ms += step) {
    ticks.push({ frame: origin + ms / millisecondsPerFrame, label: formatMilliseconds(ms, precision) });
  }
  return ticks;
}
