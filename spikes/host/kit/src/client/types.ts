import type { Inspection } from '../timeline/index.ts';

export interface ViewerDocument {
  /** Revisions can only be compared within the producing native session. */
  sessionId: string;
  revision: number;
  size: [number, number];
  snapshot: Inspection;
  peaks: number[];
  /** Present when this binding grants an editable native session. */
  editable?: true;
}

export interface Picture {
  frame: number;
  generation: number;
  revision: number;
  width: number;
  height: number;
  rgba: Uint8Array;
}

/** Sorted, disjoint, half-open intervals in project frames. */
export type OutputDamage = { kind: 'none' | 'all' } | { kind: 'ranges'; ranges: [number, number][] };

/** A placement in the newly accepted inspection and half-open project-frame intervals. */
export interface OccurrenceRanges {
  path: string;
  ranges: [number, number][];
}

export type TimelineChange =
  | { kind: 'known'; occurrences: OccurrenceRanges[]; thumbnails: OccurrenceRanges[] }
  | { kind: 'unknown' };

/** Outputs which may differ between two accepted revisions. */
export interface RevisionChange {
  fromRevision: number;
  toRevision: number;
  damage: { picture: OutputDamage; audio: OutputDamage };
  /** Directly affected rows and the isolated samples that can require regeneration. */
  timeline: TimelineChange;
}

export type ReopenedDocument = ViewerDocument & { parked: Picture; change: RevisionChange };

export interface LayerRef {
  occurrence: string;
  layers: number[];
}

export type EditorTarget =
  | ({ kind: 'layer' } & LayerRef)
  | { kind: 'placement'; occurrence: string }
  | { kind: 'track'; scope: string; track: string };

export function sameEditorTarget(a?: EditorTarget | null, b?: EditorTarget | null) {
  if (!a || !b) return !a && !b;
  if (a.kind === 'track') return b.kind === 'track' && a.scope === b.scope && a.track === b.track;
  if (a.kind === 'placement') return b.kind === 'placement' && a.occurrence === b.occurrence;
  return b.kind === 'layer' && a.occurrence === b.occurrence &&
    a.layers.length === b.layers.length && a.layers.every((value, index) => value === b.layers[index]);
}

export interface LayerGeometry {
  target: LayerRef;
  visibility: 'visible' | 'disabled' | 'inactive' | 'empty' | 'indeterminate';
  localFrame?: number;
  localBounds?: [number, number, number, number];
  localToComposition?: [[number, number, number], [number, number, number]];
  compositionQuad?: [[number, number], [number, number], [number, number], [number, number]];
  clips?: Array<{
    localToComposition: [[number, number, number], [number, number, number]];
    maskBounds?: [number, number, number, number];
    wipeRight?: number;
    empty: boolean;
  }>;
}

export interface EditorState {
  sessionId: string;
  revision: number;
  hudVersion: number;
  frame: number;
  scope: string;
  selection: EditorTarget | null;
  selections: EditorTarget[];
  selectionSource: { file: string; layers: number[] } | null;
  hud: LayerGeometry | null;
  marquee: [number, number, number, number] | null;
  hover: LayerRef | null;
  tracks: { scope: string; track: string; enabled: boolean; editable: boolean }[];
  dirty: boolean;
  conflict: boolean;
  canUndo: boolean;
  canRedo: boolean;
}

export type EditorCommand =
  | { op: 'getState' }
  | { op: 'inspect' }
  | { op: 'clearSelection' }
  | { op: 'seek'; frame: number }
  | { op: 'enterScope'; revision: number; scope: string }
  | { op: 'hitTest'; revision: number; frame: number; point: [number, number] }
  | { op: 'selectAt'; revision: number; frame: number; point: [number, number] }
  | { op: 'beginMarquee'; revision: number; frame: number; point: [number, number]; threshold?: number }
  | { op: 'updateMarquee'; revision: number; point: [number, number] }
  | { op: 'finishMarquee'; revision: number; point: [number, number] }
  | { op: 'cancelMarquee'; revision: number }
  | { op: 'hoverAt'; revision: number; frame: number; point: [number, number] }
  | { op: 'clearHover'; revision: number }
  | { op: 'layerGeometry'; revision: number; frame: number; layer: LayerRef }
  | { op: 'select'; revision: number; target: EditorTarget }
  | { op: 'setTrackEnabled'; revision: number; scope: string; track: string; enabled: boolean }
  | { op: 'undo'; revision: number }
  | { op: 'redo'; revision: number }
  | { op: 'save'; revision: number }
  | { op: 'reload'; revision: number; discardChanges?: boolean }
  | { op: 'render'; revision: number; frame: number; points?: [number, number][] };

export interface EditorEditResult {
  state: EditorState;
  change: RevisionChange | null;
}

type ResultForEditorOp<Op extends EditorCommand['op']> =
  Op extends 'inspect' ? Inspection :
  Op extends 'hitTest' ? { hits: LayerRef[]; indeterminate: boolean; obstruction?: LayerRef; fallbackPlacement?: string } :
  Op extends 'layerGeometry' ? LayerGeometry :
  Op extends 'setTrackEnabled' | 'undo' | 'redo' | 'reload' ? EditorEditResult :
  Op extends 'render' ? { revision: number; frame: number; size: [number, number]; pixels: number[][] } :
  EditorState;

export type EditorResult<C extends EditorCommand> = ResultForEditorOp<C['op']>;

export type EditorReply<C extends EditorCommand = EditorCommand> =
  | { result: EditorResult<C>; transition?: ReopenedDocument }
  | { error: { code: string; message: string } };

/** An already rasterized, transparent region in picture coordinates. */
export type HudTile = Picture & { x: number; y: number };
export interface HudSample { hudVersion: number; tile: HudTile | null }

export type PlaybackSample = { picture: Picture | null; playing: boolean };

export interface ExportReport {
  frames: number;
  audio_sample_frames: number;
  audio_sample_rate: number;
  rgba_sha256: string;
}

export type ExportStatus =
  | { state: 'idle' | 'cancelled' }
  | { state: 'running' | 'cancelling'; rendered: number; frames: number }
  | { state: 'completed'; path: string; report: ExportReport }
  | { state: 'failed'; error: string };

/** Operations available to a renderer for the project granted by its parent window. */
export interface ViewerBridge {
  /** Disk inputs have settled. Only a successful reopen advances the revision. */
  onProjectChange?(listener: () => void): () => void;
  inspect(): Promise<ViewerDocument>;
  seek(frame: number, generation: number, previewOnly?: boolean): Promise<Picture>;
  /** Optionally sample a single occurrence at its project frame. */
  thumbnail(frame: number, generation: number, placement?: string): Promise<Picture | null>;
  play(frame: number, generation: number): Promise<void>;
  next(generation: number): Promise<PlaybackSample>;
  presented(frame: number, generation: number): Promise<void>;
  pause(generation: number): Promise<void>;
  reopen(frame?: number): Promise<ReopenedDocument>;
  editor?<C extends EditorCommand>(command: C): Promise<EditorReply<C>>;
  hud?(revision: number, frame: number): Promise<HudSample>;
}
