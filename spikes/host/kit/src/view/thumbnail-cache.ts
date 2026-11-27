import type { TimelineChange } from '../client/types.ts';

interface Entry<T> {
  key: string;
  path: string;
  frame: number;
  start: number;
  end: number;
  cleanStart: number;
  cleanEnd: number;
  revision: number;
  value?: T | null;
}

/** Keys address the sampled frame of one isolated occurrence, including offscreen residents. */
export class ThumbnailCache<T> {
  private entries = new Map<string, Entry<T>>();
  private residents = new Map<string, Set<Entry<T>>>();
  private revision = 0;
  private readonly limit: number;

  constructor(limit: number) { this.limit = limit; }

  get(key: string) { return this.entries.get(key)?.value; }
  has(key: string) { return this.entries.has(key); }
  accepts(entry: Entry<T>) { return this.entries.get(entry.key) === entry; }

  /** While the exact sample loads, borrow a representative from the same occurrence. */
  forDisplay(key: string, path: string, frame: number, start: number, end: number): T | null | undefined {
    return this.displayEntry(key, path, frame, start, end)?.value;
  }

  private displayEntry(key: string, path: string, frame: number, start: number, end: number) {
    const exact = this.entries.get(key);
    if (exact?.value !== undefined) return exact;

    let closest: Entry<T> | undefined;
    let distance = Infinity;
    for (const entry of this.residents.get(path) ?? []) {
      // A previous revision can still represent a cell on its clean side of an edit.
      if (start < entry.cleanStart || end > entry.cleanEnd) continue;
      const oldCellContainsFrame = entry.start <= frame && frame < entry.end;
      const newCellContainsSample = start <= entry.frame && entry.frame < end;
      if (!oldCellContainsFrame && !newCellContainsSample) continue;
      const delta = Math.abs(entry.frame - frame);
      if (delta < distance || (delta === distance && closest && entry.frame < closest.frame)) {
        closest = entry;
        distance = delta;
      }
    }
    return closest;
  }

  begin(key: string, path: string, frame: number, start = frame, end = frame + 1): Entry<T> {
    const previous = this.entries.get(key);
    if (previous) this.remove(previous);
    const entry = {key, path, frame, start, end, cleanStart: -Infinity, cleanEnd: Infinity, revision: this.revision};
    this.entries.set(key, entry);
    return entry;
  }

  complete(entry: Entry<T>, value: T | null) {
    if (!this.accepts(entry) || entry.value !== undefined) return;
    // A reopen may have cancelled a pending native request for an otherwise clean tile.
    if (value === null && entry.revision !== this.revision) this.entries.delete(entry.key);
    else {
      entry.value = value;
      if (value !== null) {
        const residents = this.residents.get(entry.path) ?? new Set<Entry<T>>();
        residents.add(entry);
        this.residents.set(entry.path, residents);
      }
    }
  }

  accept(change: TimelineChange | undefined, contains: (path: string, frame: number) => boolean) {
    this.revision++;
    if (change?.kind !== 'known') {
      this.clear();
      return;
    }
    const changed = new Map(change.thumbnails.map(({path, ranges}) => [path, ranges]));
    for (const entry of this.entries.values()) {
      if (!contains(entry.path, entry.frame)) {
        this.remove(entry);
        continue;
      }
      for (const [lo, hi] of changed.get(entry.path) ?? []) {
        if (entry.frame < lo) entry.cleanEnd = Math.min(entry.cleanEnd, lo);
        else if (entry.frame >= hi) entry.cleanStart = Math.max(entry.cleanStart, hi);
        else {
          this.remove(entry);
          break;
        }
      }
    }
  }

  trim(wanted: ReadonlyMap<string, { placement: { path: string }; frame: number; start: number; end: number }>) {
    if (this.entries.size <= this.limit) return;
    const keep = new Set(wanted.keys());
    for (const [key, tile] of wanted) {
      const display = this.displayEntry(key, tile.placement.path, tile.frame, tile.start, tile.end);
      if (display) keep.add(display.key);
    }
    for (const entry of this.entries.values()) {
      if (this.entries.size <= this.limit) break;
      if (!keep.has(entry.key)) this.remove(entry);
    }
  }

  private remove(entry: Entry<T>) {
    this.entries.delete(entry.key);
    const residents = this.residents.get(entry.path);
    if (residents?.delete(entry) && !residents.size) this.residents.delete(entry.path);
  }

  clear() {
    this.entries.clear();
    this.residents.clear();
  }
}
