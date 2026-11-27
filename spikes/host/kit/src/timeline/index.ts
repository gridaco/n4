import type { EditorState, EditorTarget, RevisionChange } from '../client/types.ts';
import { sameEditorTarget } from '../client/types.ts';

export { formatElapsedTime, timelineTicks } from './time.ts';

// The structural subset of the Rust Inspection document used by the timeline.
export type FrameRange = [number, number];
export interface Inspection {
  name: string;
  duration: number;
  fps: [number, number];
  placements: Placement[];
  definitions: Record<string, { layers: Layer[] }>;
}
export interface Placement {
  path: string;
  name: string;
  track: string;
  local: FrameRange;
  project: FrameRange;
  enabled?: boolean;
  definition?: string;
  children?: Placement[];
}
export interface Layer {
  name: string;
  kind: string;
  authoredPath?: number[];
  start: number;
  end: number | null;
  children: Layer[];
  tracks: { name: string; keys: { frame: number; value: number }[] }[];
}
export interface SeekIntent {
  frame: number;
  phase: 'scrub' | 'exact';
}
export interface TimelineEditorActions {
  select(target: EditorTarget): void;
  enterScope(scope: string): void;
  setTrackEnabled(scope: string, track: string, enabled: boolean): void;
}
export interface Clip {
  placement: Placement;
  x: number;
  right: number;
  changedRanges: FrameRange[];
  selected?: boolean;
}
export interface KeyMark {
  frame: number;
  value: number;
  x: number;
}
export interface Row {
  id: string;
  kind: 'lane' | 'layer' | 'property' | 'mix';
  label: string;
  depth: number;
  top: number;
  height: number;
  expandable: boolean;
  expanded: boolean;
  clips: Clip[];
  keys: KeyMark[];
  range?: FrameRange;
  selected?: boolean;
  target?: EditorTarget;
  track?: EditorState['tracks'][number];
}
export type Hit =
  | { kind: 'clip'; clip: Clip }
  | { kind: 'key'; row: Row; key: KeyMark }
  | { kind: 'row'; row: Row }
  | { kind: 'time'; frame: number }
  | { kind: TimelineAction };

export type TimelineAction = 'up' | 'fit' | 'closer' | 'wider' | 'nextUse';

export interface TimelineLayout {
  width: number;
  height: number;
  gutter: number;
  rows: Row[];
  /** Continuous project-frame coordinates, including partially visible frames. */
  visible: FrameRange;
  pixelsPerFrame: number;
  scope: FrameRange;
  scopePath: string;
  rulerChanges: FrameRange[];
  breadcrumb: string;
  use: string | null;
  buttons: {
    action: 'fit' | 'closer' | 'wider' | 'nextUse';
    label: string;
    x: number;
    width: number;
  }[];
}
export const headerHeight = 28;
export const rulerHeight = 26;
export const rowHeight = 28;
export const trackHeight = 56;
export const bodyTop = headerHeight + rulerHeight;

function clamp(n: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(n, hi));
}

function union(ranges: FrameRange[]): FrameRange[] {
  const merged: FrameRange[] = [];
  for (const [start, end] of ranges.toSorted((a, b) => a[0] - b[0])) {
    const previous = merged.at(-1);
    if (previous && start <= previous[1]) previous[1] = Math.max(previous[1], end);
    else merged.push([start, end]);
  }
  return merged;
}

// Layer and property row IDs are ordinal within one occurrence. Keep an
// inspector row across a save only while that address still describes it.
function sameLayerRow(id: string, before: Layer[], after: Layer[], scope: FrameRange, frame?: number) {
  const parts = id.split('/').slice(1);
  for (let depth = 0; depth < parts.length; depth++) {
    const index = Number(parts[depth]);
    const old = before[index], current = after[index];
    if (!old || !current || old.kind !== current.kind || old.name !== current.name) return false;
    const property = parts[depth + 1];
    if (property?.startsWith('t')) {
      const trackIndex = Number(property.slice(1));
      const track = current.tracks[trackIndex];
      const end = current.end === null ? scope[1] : Math.min(scope[1], scope[0] + current.end);
      return !!track && track.name === old.tracks[trackIndex]?.name &&
        (frame === undefined || frame < end &&
          track.keys.some(key => scope[0] + current.start + key.frame === frame));
    }
    before = old.children;
    after = current.children;
  }
  return true;
}

/** Resolve a nested selection to the clip visible in the current timeline scope. */
function clipUnderScope(target: EditorTarget | null, scope: string) {
  if (!target || target.kind === 'track') return '';
  const prefix = scope ? scope + '/' : '';
  if (!target.occurrence.startsWith(prefix)) return '';
  const end = target.occurrence.indexOf('/', prefix.length);
  return end < 0 ? target.occurrence : target.occurrence.slice(0, end);
}

function locateLayer(layers: Layer[], address: number[]) {
  let row = 'layer';
  const ancestry: string[] = [];
  const names: string[] = [];
  let layer: Layer | undefined;
  for (let depth = 0; depth < address.length; depth++) {
    const index = layers.findIndex(candidate =>
      candidate.authoredPath?.length === depth + 1 &&
      candidate.authoredPath.every((value, i) => value === address[i]));
    layer = layers[index];
    if (!layer) return undefined;
    if (depth) ancestry.push(row);
    row += '/' + index;
    names.push(layer.name);
    layers = layer.children;
  }
  return layer && { row, layer, ancestry, label: names.join(' › ') };
}

export class TimelineController {
  private snapshot: Inspection;
  private sendSeek: (intent: SeekIntent) => void;
  private sendSkim?: (frame: number | null) => void;
  private skimmingEnabled = true;
  private hoverFrame: number | null = null;
  private placements = new Map<string, Placement>();
  private uses = new Map<string, Placement[]>();
  private occurrenceChanges = new Map<string, FrameRange[]>();
  private unattributedChanges: FrameRange[] = [];
  private scopePath = '';
  private openLayers = new Set<string>();
  private first = 0;
  private span: number;
  private width = 800;
  private height = 260;
  private scroll = 0;
  private rowsHeight = 0;
  private listeners = new Set<() => void>();
  private cached?: TimelineLayout;
  private drag = false;
  private showMix = false;
  private editor?: TimelineEditorActions;
  private editorState?: EditorState;
  private revealSelection = false;
  cursor = 0;
  selection: { id: string; frame?: number } | null = null;

  constructor(
    snapshot: Inspection,
    onSeek: (intent: SeekIntent) => void,
    onSkim?: (frame: number | null) => void,
    editor?: TimelineEditorActions,
  ) {
    this.snapshot = snapshot;
    this.sendSeek = onSeek;
    this.sendSkim = onSkim;
    this.editor = editor;
    this.span = snapshot.duration;
    this.index();
  }

  private index() {
    this.placements.clear();
    this.uses.clear();
    const visit = (placement: Placement) => {
      this.placements.set(placement.path, placement);
      if (placement.definition) {
        const uses = this.uses.get(placement.definition) ?? [];
        uses.push(placement);
        this.uses.set(placement.definition, uses);
      }
      placement.children?.forEach(visit);
    };
    this.snapshot.placements.forEach(visit);
  }

  private changed(layout = true) {
    if (layout) this.cached = undefined;
    this.listeners.forEach((fn) => fn());
  }

  /** Project the accepted native selection and track state into the viewport. */
  setEditorState(state: EditorState) {
    const prior = this.editorState;
    if (prior?.sessionId === state.sessionId && prior.hudVersion > state.hudVersion) return;
    this.editorState = state;
    const scope = state.scope === 'root' ? '' : state.scope;
    const scopeChanged = scope !== this.scopePath;
    if (scopeChanged) this.resetScope(scope);
    const frame = clamp(state.frame, 0, this.duration - 1);
    const frameChanged = !this.drag && this.cursor !== frame;
    if (frameChanged) this.cursor = frame;
    const selectionChanged = prior?.selections.length !== state.selections.length ||
      state.selections.some((target, index) => !sameEditorTarget(prior?.selections[index], target));
    if ((selectionChanged || scopeChanged) && state.selection) {
      this.revealSelection = true;
      for (const target of state.selections) {
        if (target.kind === 'layer' && target.occurrence === scope) {
          const found = this.findLayer(target);
          for (const id of found?.ancestry ?? []) this.openLayers.add(id);
        }
      }
      const target = state.selection;
      if (target.kind !== 'track') {
        const placement = this.placements.get(target.occurrence);
        if (placement && placement.project[0] < this.scopeRange()[1] &&
            placement.project[1] > this.scopeRange()[0] &&
            (placement.project[0] >= this.last || placement.project[1] <= this.first)) {
          const [lo, hi] = this.scopeRange();
          this.first = clamp(placement.project[0], lo, hi - this.span);
        }
      }
    }
    const tracksChanged = !prior || prior.tracks.length !== state.tracks.length ||
      state.tracks.some((track, i) => {
        const previous = prior.tracks[i];
        return track.scope !== previous.scope || track.track !== previous.track ||
          track.editable !== previous.editable || track.enabled !== previous.enabled;
      });
    if (scopeChanged || selectionChanged || tracksChanged) this.changed();
    else if (frameChanged) this.changed(false);
  }

  private findLayer(target: Extract<EditorTarget, { kind: 'layer' }>) {
    const definition = this.placements.get(target.occurrence)?.definition;
    const layers = definition && this.snapshot.definitions[definition]?.layers;
    return layers ? locateLayer(layers, target.layers) : undefined;
  }

  subscribe(listener: () => void) {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  setAudioMix(mix: boolean) {
    this.showMix = mix;
    if (!mix && this.selection?.id === 'mix') this.selection = null;
    this.changed();
  }

  get dragging() {
    return this.drag;
  }
  get skimFrame() {
    return this.hoverFrame;
  }
  get duration() {
    return this.snapshot.duration;
  }
  get fps() {
    return this.snapshot.fps;
  }

  setSkimmingEnabled(enabled: boolean) {
    this.skimmingEnabled = enabled;
    if (!enabled) this.clearSkim();
  }

  skimAt(x: number) {
    if (!this.sendSkim || !this.skimmingEnabled || this.drag) return;
    const frame = this.frameAt(x);
    if (this.hoverFrame === frame) return;
    this.hoverFrame = frame;
    this.changed(false);
    this.sendSkim(frame);
  }

  clearSkim() {
    if (this.hoverFrame === null) return;
    this.hoverFrame = null;
    this.changed(false);
    this.sendSkim?.(null);
  }

  containsSample(path: string, frame: number): boolean {
    const place = this.placements.get(path);
    return !!place && place.project[0] <= frame && frame < place.project[1];
  }

  private review(change?: RevisionChange) {
    this.occurrenceChanges.clear();
    this.unattributedChanges = [];
    if (!change) return;
    if (change.timeline.kind === 'unknown') {
      const ranges: FrameRange[] = [];
      for (const output of [change.damage.picture, change.damage.audio]) {
        if (output.kind === 'all') ranges.push([0, this.duration]);
        if (output.kind === 'ranges') ranges.push(...output.ranges);
      }
      this.unattributedChanges = union(ranges);
      return;
    }
    const direct = new Map(change.timeline.occurrences.map(({path, ranges}) => [path, ranges]));
    if (!direct.size) return;
    // A collapsed group summarizes its children without marking its other children or siblings.
    const collect = (place: Placement): FrameRange[] => {
      const ranges = union([...(direct.get(place.path) ?? []), ...(place.children ?? []).flatMap(collect)]);
      if (ranges.length) this.occurrenceChanges.set(place.path, ranges);
      return ranges;
    };
    this.snapshot.placements.forEach(collect);
  }

  replace(snapshot: Inspection, change?: RevisionChange) {
    const range = this.scopeRange();
    const fitted = this.first === range[0] && this.span === range[1] - range[0];
    const prior = this.placements.get(this.scopePath);
    const before = prior?.definition ? this.snapshot.definitions[prior.definition]?.layers ?? [] : [];
    this.snapshot = snapshot;
    this.index();
    this.review(change);
    this.cursor = clamp(this.cursor, 0, snapshot.duration - 1);
    if (this.scopePath && !this.placements.has(this.scopePath)) {
      let scope = this.scopePath;
      do {
        const slash = scope.lastIndexOf('/');
        scope = slash < 0 ? '' : scope.slice(0, slash);
      } while (scope && !this.placements.has(scope));
      this.resetScope(scope);
      this.changed();
      return;
    }
    this.clearSkim();
    const scope = this.scopeRange();
    const [lo, hi] = scope;
    this.span = fitted ? hi - lo : Math.min(hi - lo, this.span);
    this.first = clamp(this.first, lo, hi - this.span);
    const place = this.placements.get(this.scopePath);
    const after = place?.definition ? snapshot.definitions[place.definition]?.layers ?? [] : [];
    for (const id of this.openLayers)
      if (!sameLayerRow(id, before, after, scope)) this.openLayers.delete(id);
    if (this.selection) {
      const { id, frame } = this.selection;
      const children = place ? place.children ?? [] : snapshot.placements;
      const keep = id.startsWith('layer/') ? sameLayerRow(id, before, after, scope, frame) :
        id.startsWith('lane/') ? children.some(child => 'lane/' + child.track === id) :
        id === 'mix' || children.some(child => child.path === id);
      if (!keep) this.selection = null;
    }
    this.changed();
  }

  resize(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    this.changed();
  }

  private scopeRange(): FrameRange {
    return this.placements.get(this.scopePath)?.project ?? [0, this.duration];
  }

  private get gutter() {
    return Math.min(210, Math.floor(this.width * 0.31));
  }

  private get last() {
    return Math.min(this.first + this.span, this.scopeRange()[1]);
  }

  private get pixelsPerFrame() {
    return (this.width - this.gutter) / this.span;
  }

  enter(path: string) {
    if (path && !this.placements.has(path)) return false;
    if (this.editor) this.editor.enterScope(path || 'root');
    else {
      this.resetScope(path);
      this.changed();
    }
    return true;
  }

  private resetScope(path: string) {
    this.scopePath = path;
    this.openLayers.clear();
    this.selection = null;
    this.drag = false;
    this.scroll = 0;
    this.clearSkim();
    const [lo, hi] = this.scopeRange();
    this.first = lo;
    this.span = hi - lo;
  }

  up() {
    const slash = this.scopePath.lastIndexOf('/');
    this.enter(slash < 0 ? '' : this.scopePath.slice(0, slash));
  }

  fit() {
    const [lo, hi] = this.scopeRange();
    this.first = lo;
    this.span = hi - lo;
    this.changed();
  }

  frameToX(frame: number) {
    return this.gutter + (frame - this.first) * this.pixelsPerFrame;
  }

  frameAt(x: number) {
    const frame = this.first + (x - this.gutter) / this.pixelsPerFrame;
    const last = this.last;
    // Keep floating-point error from turning an exact boundary into the previous frame.
    const epsilon = 8 * Number.EPSILON * Math.max(1, last);
    return clamp(Math.floor(frame + epsilon), Math.floor(this.first + epsilon), Math.ceil(last - epsilon) - 1);
  }

  /** A single-frame edit stays visible when zoomed out. */
  markBounds([start, end]: FrameRange, clip?: Clip): [number, number] | null {
    if (end <= this.first || start >= this.last) return null;
    const right = clip?.right ?? this.width;
    const left = clamp(this.frameToX(start), clip?.x ?? this.gutter, right);
    const width = Math.min(right - (clip?.x ?? this.gutter), Math.max(3, Math.min(right, this.frameToX(end)) - left));
    const x = Math.min(left, right - width);
    return [x, x + width];
  }

  zoom(factor: number, at = this.cursor) {
    this.zoomAt(factor, this.frameToX(at));
  }

  zoomAt(factor: number, x: number) {
    const [lo, hi] = this.scopeRange();
    const next = clamp(this.span * factor, 1, hi - lo);
    if (next === this.span) return;
    const fraction = clamp((x - this.gutter) / (this.width - this.gutter), 0, 1);
    this.first = clamp(this.first + fraction * (this.span - next), lo, hi - next);
    this.span = next;
    this.changed();
  }

  pan(pixels: number) {
    const [lo, hi] = this.scopeRange();
    const first = clamp(this.first + pixels / this.pixelsPerFrame, lo, hi - this.span);
    if (first === this.first) return;
    this.first = first;
    this.changed();
  }

  scrollRows(pixels: number) {
    this.layout();
    this.scroll = clamp(this.scroll + pixels, 0, this.maxScroll());
    this.changed();
  }

  private maxScroll() {
    return Math.max(0, this.rowsHeight - Math.max(0, this.height - bodyTop));
  }

  toggleLayer(id: string) {
    if (this.openLayers.has(id)) this.openLayers.delete(id);
    else this.openLayers.add(id);
    this.changed();
  }

  layout(): TimelineLayout {
    if (this.cached) return this.cached;
    const place = this.placements.get(this.scopePath);
    const scope = this.scopeRange();
    const last = this.last;
    const uses = place?.definition ? this.uses.get(place.definition)! : [];
    const use = uses.length > 1 ? uses.indexOf(place!) + 1 + '/' + uses.length : null;
    const rows: Row[] = [];
    const selected = this.editorState?.selections ?? [];
    const markedClips = new Set(selected.map(target => clipUnderScope(target, this.scopePath)));
    const inlineLayers = new Map<string, Extract<EditorTarget, { kind: 'layer' }>[]>();
    for (const target of selected) {
      if (target.kind !== 'layer' || target.occurrence === this.scopePath) continue;
      const owner = clipUnderScope(target, this.scopePath);
      const list = inlineLayers.get(owner);
      if (list) list.push(target);
      else inlineLayers.set(owner, [target]);
    }
    let rowsHeight = 0;
    const makeRow = (kind: Row['kind'], id: string, label: string, depth = 0): Row => {
      const height = kind === 'mix' ? 42 : kind === 'lane' && label !== 'CAM' ? trackHeight : rowHeight;
      const row: Row = {
        kind,
        id,
        label,
        depth,
        top: bodyTop + rowsHeight,
        height,
        clips: [],
        keys: [],
        expandable: false,
        expanded: false,
      };
      rowsHeight += height;
      rows.push(row);
      return row;
    };
    const lanes = new Map<string, Placement[]>();
    for (const p of place ? (place.children ?? []) : this.snapshot.placements) {
      const lane = lanes.get(p.track) ?? [];
      lane.push(p);
      lanes.set(p.track, lane);
    }
    const orderedLanes = [...lanes].sort(([a], [b]) =>
      Number(a.startsWith('A')) - Number(b.startsWith('A')));
    const addInlineLayers = (owner: string) => {
      for (const target of inlineLayers.get(owner) ?? []) {
        const layer = this.findLayer(target);
        const placement = this.placements.get(target.occurrence);
        if (!layer || !placement) continue;
        const row = makeRow('layer', `picked-layer/${target.occurrence}/${target.layers.join('.')}`, layer.label, 1);
        row.target = target;
        row.selected = true;
        row.range = [
          placement.project[0] + layer.layer.start,
          Math.min(placement.project[1], placement.project[0] + (layer.layer.end ?? placement.local[1] - placement.local[0])),
        ];
      }
    };
    for (const [lane, placements] of orderedLanes) {
      const row = makeRow('lane', 'lane/' + lane, lane);
      if (this.editor) {
        const scope = this.scopePath || 'root';
        row.target = { kind: 'track', scope, track: lane };
        row.selected = selected.some(target => sameEditorTarget(row.target, target));
        row.track = this.editorState?.tracks.find(track => track.scope === scope && track.track === lane);
      }
      for (const p of placements) {
        const [lo, hi] = p.project;
        if (hi <= this.first || lo >= last) continue;
        row.clips.push({
          placement: p,
          changedRanges: this.occurrenceChanges.get(p.path) ?? [],
          x: this.frameToX(Math.max(lo, this.first)),
          right: this.frameToX(Math.min(hi, last)),
          selected: markedClips.has(p.path),
        });
      }
      for (const placement of placements) addInlineLayers(placement.path);
    }
    addInlineLayers('');
    if (this.showMix) makeRow('mix', 'mix', 'Audio mix');
    const addLayers = (layers: Layer[], prefix: string, depth: number) => {
      layers.forEach((layer, index) => {
        const id = prefix + '/' + index;
        const row = makeRow('layer', id, layer.name, depth);
        if (this.editor && layer.authoredPath) {
          row.target = { kind: 'layer', occurrence: this.scopePath, layers: layer.authoredPath };
          row.selected = selected.some(target => sameEditorTarget(row.target, target));
        }
        const start = scope[0] + layer.start;
        const end = Math.min(scope[1], layer.end === null ? scope[1] : scope[0] + layer.end);
        row.range = [start, end];
        row.expandable = layer.children.length + layer.tracks.length > 0;
        row.expanded = this.openLayers.has(id);
        if (!row.expanded) return;
        layer.tracks.forEach((track, i) => {
          const property = makeRow('property', id + '/t' + i, track.name, depth + 1);
          property.target = row.target;
          for (const k of track.keys) {
            const frame = start + k.frame;
            if (frame >= end || frame < this.first || frame >= last) continue;
            property.keys.push({ frame, value: k.value, x: this.frameToX(frame) });
          }
        });
        addLayers(layer.children, id, depth + 1);
      });
    };
    if (place?.definition)
      addLayers(this.snapshot.definitions[place.definition]?.layers ?? [], 'layer', 0);
    this.rowsHeight = rowsHeight;
    this.scroll = clamp(this.scroll, 0, this.maxScroll());
    if (this.revealSelection) {
      const marked = rows.filter(row => row.selected || row.clips.some(clip => clip.selected));
      let first = marked[0], last = marked.at(-1);
      if (first && last && last.top + last.height - first.top > this.height - bodyTop) {
        const primary = this.editorState?.selection ?? null;
        const clip = clipUnderScope(primary, this.scopePath);
        const owner = marked.find(row => row.clips.some(item => item.placement.path === clip));
        const active = marked.find(row => row.target && sameEditorTarget(row.target, primary)) ?? owner;
        if (active) {
          first = owner && active.top + active.height - owner.top <= this.height - bodyTop ? owner : active;
          last = active;
        }
      }
      if (first && last) {
        const top = first.top - bodyTop;
        this.scroll = clamp(this.scroll, Math.min(top, last.top + last.height - this.height), top);
        this.scroll = clamp(this.scroll, 0, this.maxScroll());
      }
      this.revealSelection = false;
    }
    const visibleRows = rows.filter((row) => {
      row.top -= this.scroll;
      return row.top < this.height && row.top + row.height > bodyTop;
    });
    const buttons: TimelineLayout['buttons'] = [];
    let right = this.width - 5;
    const button = (action: (typeof buttons)[number]['action'], label: string, width: number) => {
      right -= width;
      buttons.push({ action, label, x: right, width });
      right -= 2;
    };
    button('fit', 'Fit', 32);
    button('closer', '+', 26);
    button('wider', '−', 26);
    if (use) button('nextUse', 'Use ' + use, 62);
    const names = [];
    let path = this.scopePath;
    while (path) {
      names.unshift(this.placements.get(path)?.name ?? path);
      const slash = path.lastIndexOf('/');
      path = slash < 0 ? '' : path.slice(0, slash);
    }
    this.cached = {
      width: this.width,
      height: this.height,
      gutter: this.gutter,
      rows: visibleRows,
      scopePath: this.scopePath,
      rulerChanges: this.occurrenceChanges.get(this.scopePath) ?? this.unattributedChanges,
      scope,
      visible: [this.first, last],
      pixelsPerFrame: this.pixelsPerFrame,
      use,
      buttons,
      breadcrumb: [this.snapshot.name, ...names].join('  ›  '),
    };
    return this.cached;
  }

  hitTest(x: number, y: number): Hit | null {
    const layout = this.layout();
    if (x < 0 || x >= this.width || y < 0 || y >= this.height) return null;
    if (y < headerHeight) {
      const btn = layout.buttons.find((b) => x >= b.x && x < b.x + b.width);
      if (btn) return { kind: btn.action };
      return this.scopePath && x < 32 ? { kind: 'up' } : null;
    }
    const time: Hit = { kind: 'time', frame: this.frameAt(x) };
    if (y < bodyTop) return x < this.gutter ? null : time;
    const row = layout.rows.find((r) => y >= r.top && y < r.top + r.height);
    if (!row) return x >= this.gutter ? time : null;
    if (x < this.gutter) return { kind: 'row', row };
    // Use the closest key when multiple handles share a few screen pixels.
    let key: KeyMark | undefined;
    for (const candidate of row.keys) {
      if (
        Math.abs(candidate.x - x) <= 6 &&
        (!key || Math.abs(candidate.x - x) < Math.abs(key.x - x))
      )
        key = candidate;
    }
    if (key) return { kind: 'key', row, key };
    let clip: Clip | undefined;
    for (const candidate of row.clips)
      if (x >= candidate.x && x < candidate.right) clip = candidate;
    if (!clip) return time;
    return { kind: 'clip', clip };
  }

  private seek(frame: number, phase: SeekIntent['phase']) {
    this.hoverFrame = null;
    this.cursor = clamp(frame, 0, this.duration - 1);
    this.changed(false);
    this.sendSeek({ frame: this.cursor, phase });
  }

  setPlaybackFrame(frame: number) {
    if (this.drag) return;
    this.hoverFrame = null;
    this.cursor = clamp(frame, 0, this.duration - 1);
    this.changed(false);
  }

  selectRow(id: string) {
    const row = this.layout().rows.find((item) => item.id === id);
    if (!row) return;
    if (this.editor) {
      if (row.target) this.editor.select(row.target);
    } else this.selection = { id };
    if (row.expandable) this.toggleLayer(id);
    else this.changed(false);
  }

  toggleTrack(id: string) {
    const track = this.layout().rows.find(row => row.id === id)?.track;
    if (track?.editable) this.editor?.setTrackEnabled(track.scope, track.track, !track.enabled);
  }

  navigate(action: TimelineAction) {
    switch (action) {
      case 'up':
        this.up();
        break;
      case 'fit':
        this.fit();
        break;
      case 'closer':
        this.zoom(0.5);
        break;
      case 'wider':
        this.zoom(2);
        break;
      case 'nextUse': {
        const place = this.placements.get(this.scopePath);
        const uses = place?.definition && this.uses.get(place.definition);
        if (!place || !uses || uses.length < 2) return;
        const next = uses[(uses.indexOf(place) + 1) % uses.length]!;
        const local = this.cursor - place.project[0];
        this.enter(next.path);
        this.seek(clamp(next.project[0] + local, next.project[0], next.project[1] - 1), 'exact');
        break;
      }
    }
  }

  beginScrub(x: number) {
    this.drag = true;
    this.seek(this.frameAt(x), 'scrub');
  }

  pointerDown(x: number, y: number, clicks = 1) {
    const hit = this.hitTest(x, y);
    if (!hit) return;
    switch (hit.kind) {
      case 'up':
      case 'fit':
      case 'closer':
      case 'wider':
      case 'nextUse':
        this.navigate(hit.kind);
        break;
      case 'row':
        this.selectRow(hit.row.id);
        break;
      case 'clip': {
        const p = hit.clip.placement;
        const frame = p.track === 'CAM' ? p.project[0] : this.frameAt(x);
        if (clicks >= 2 && p.track !== 'CAM') this.enter(p.path);
        else if (this.editor) this.editor.select({ kind: 'placement', occurrence: p.path });
        else this.selection = { id: p.path };
        this.seek(frame, 'exact');
        break;
      }
      case 'key':
        if (this.editor) {
          if (hit.row.target) this.editor.select(hit.row.target);
        } else this.selection = { id: hit.row.id, frame: hit.key.frame };
        this.seek(hit.key.frame, 'exact');
        break;
      case 'time':
        this.beginScrub(x);
        break;
    }
  }

  pointerMove(x: number) {
    if (this.drag && this.frameAt(x) !== this.cursor) this.seek(this.frameAt(x), 'scrub');
  }

  cancelDrag() {
    if (this.drag) {
      this.drag = false;
      this.changed(false);
    }
  }

  pointerUp(x?: number) {
    if (!this.drag) return;
    this.drag = false;
    this.seek(x === undefined ? this.cursor : this.frameAt(x), 'exact');
  }

  keyboard(key: string) {
    const [start, end] = this.scopeRange();
    switch (key) {
      case 'ArrowLeft':
        this.seek(clamp(this.cursor - 1, start, end - 1), 'exact');
        break;
      case 'ArrowRight':
        this.seek(clamp(this.cursor + 1, start, end - 1), 'exact');
        break;
      case 'ArrowDown':
        this.scrollRows(rowHeight);
        break;
      case 'ArrowUp':
        this.scrollRows(-rowHeight);
        break;
      case 'PageDown':
        this.scrollRows(this.height - bodyTop);
        break;
      case 'PageUp':
        this.scrollRows(bodyTop - this.height);
        break;
      case 'Home':
        this.seek(start, 'exact');
        break;
      case 'End':
        this.seek(end - 1, 'exact');
        break;
      case '+':
      case '=':
        this.zoom(0.5);
        break;
      case '-':
        this.zoom(2);
        break;
      case 'Escape':
      case 'Backspace':
        this.up();
        break;
      case 'Enter':
        if (this.editor) {
          const selected = this.editorState?.selection;
          if (selected?.kind !== 'placement') return false;
          this.enter(selected.occurrence);
        } else {
          if (!this.selection || !this.placements.has(this.selection.id)) return false;
          this.enter(this.selection.id);
        }
        break;
      default:
        return false;
    }
    return true;
  }
}
