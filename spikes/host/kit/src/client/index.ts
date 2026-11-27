import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface } from 'node:readline';
import type { Readable, Writable } from 'node:stream';
import type {
  EditorCommand, EditorReply, EditorResult, EditorTarget, ExportReport, ExportStatus, HudSample, Picture,
  PlaybackSample, ReopenedDocument, RevisionChange, ViewerBridge, ViewerDocument,
} from './types.ts';
import type { Placement } from '../timeline/index.ts';
import { continuesSession } from './live-reload.ts';
import { sameEditorTarget } from './types.ts';

type Json = Record<string, unknown>;
type Reply = { fields: Json; rgba?: Uint8Array };
type Waiter = { resolve: (reply: Reply) => void; reject: (error: Error) => void };
export * from './types.ts';
export { LiveReload } from './live-reload.ts';

function integer(value: unknown, name: string, max = Number.MAX_SAFE_INTEGER): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0 || value > max)
    throw new Error('Invalid host ' + name);
  return value;
}

function object(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function indexPath(value: unknown): boolean {
  return Array.isArray(value) && value.every(index => Number.isSafeInteger(index) && index >= 0);
}

function range(value: unknown): boolean {
  return Array.isArray(value) && value.length === 2 &&
    value.every(v => Number.isSafeInteger(v) && v >= 0) && value[1] >= value[0];
}

function placement(value: unknown): boolean {
  return object(value) && typeof value.name === 'string' && typeof value.path === 'string' &&
    typeof value.track === 'string' && range(value.local) && range(value.project) &&
    (value.enabled === undefined || typeof value.enabled === 'boolean') &&
    (value.definition === undefined || typeof value.definition === 'string') &&
    (value.children === undefined || Array.isArray(value.children) && value.children.every(placement));
}

function layer(value: unknown): boolean {
  return object(value) && typeof value.kind === 'string' && typeof value.name === 'string' &&
    (value.authoredPath === undefined || indexPath(value.authoredPath)) &&
    Number.isSafeInteger(value.start) && (value.end === null || Number.isSafeInteger(value.end)) &&
    Array.isArray(value.children) && value.children.every(layer) && Array.isArray(value.tracks) &&
    value.tracks.every(t => object(t) && typeof t.name === 'string' && Array.isArray(t.keys) &&
      t.keys.every(k => object(k) && Number.isSafeInteger(k.frame) && Number.isFinite(k.value)));
}

function document(fields: Json): ViewerDocument {
  const { size, snapshot, peaks } = fields;
  if (fields.protocol !== 4 || typeof fields.sessionId !== 'string' || !fields.sessionId || !Array.isArray(size) || size.length !== 2 ||
      !object(snapshot) || !Array.isArray(snapshot.fps) || snapshot.fps.length !== 2 ||
      !Array.isArray(snapshot.placements) || !snapshot.placements.every(placement) || !object(snapshot.definitions) ||
      !Object.values(snapshot.definitions).every(d => object(d) && Array.isArray(d.layers) && d.layers.every(layer)) ||
      typeof snapshot.name !== 'string' || !Array.isArray(peaks) || peaks.some(v => !Number.isFinite(v)))
    throw new Error('Invalid N4 host handshake or inspection');
  const width = integer(size[0], 'width', 8192), height = integer(size[1], 'height', 8192);
  if (!width || !height || !integer(snapshot.duration, 'duration') ||
      snapshot.fps.some(v => !integer(v, 'fps'))) throw new Error('Empty N4 project profile');
  return { sessionId: fields.sessionId, revision: integer(fields.revision, 'revision'), size: [width, height],
    snapshot: snapshot as unknown as ViewerDocument['snapshot'], peaks,
    ...(fields.editable === true && { editable: true as const }) };
}

function picture(reply: Reply, expectedGeneration: number, expectedFrame?: number): Picture {
  const { fields, rgba } = reply;
  const frame = integer(fields.frame, 'frame');
  const generation = integer(fields.generation, 'generation');
  if (!rgba || generation !== expectedGeneration || (expectedFrame !== undefined && frame !== expectedFrame))
    throw new Error('Host returned a mismatched picture');
  return { frame, generation, revision: integer(fields.revision, 'revision'),
    width: Number(fields.width), height: Number(fields.height), rgba };
}

export type NativeExportStatus = Exclude<ExportStatus, { state: 'completed' }>
  | { state: 'completed'; report: ExportReport };

function validRanges(value: unknown, min: number, max: number): boolean {
  if (!Array.isArray(value) || value.length === 0) return false;
  let previousEnd = -1;
  for (const span of value) {
    if (!Array.isArray(span) || !range(span)) return false;
    const [start, end] = span;
    if (start < min || end > max || start <= previousEnd || start === end) return false;
    previousEnd = end;
  }
  return true;
}

function outputDamage(value: unknown, duration: number): boolean {
  return object(value) && (value.kind === 'all' || value.kind === 'none' ||
    value.kind === 'ranges' && validRanges(value.ranges, 0, duration));
}

function isRevisionChange(value: unknown, snapshot: ViewerDocument['snapshot']): value is RevisionChange {
  if (!object(value) || !Number.isSafeInteger(value.fromRevision) || !Number.isSafeInteger(value.toRevision) ||
      !object(value.damage) || !outputDamage(value.damage.picture, snapshot.duration) ||
      !outputDamage(value.damage.audio, snapshot.duration) || !object(value.timeline)) return false;
  const timeline = value.timeline;
  if (timeline.kind === 'unknown') return true;
  if (timeline.kind !== 'known') return false;

  const placements = new Map<string, Placement>();
  function visit(items: Placement[]): boolean {
    for (const p of items) {
      if (placements.has(p.path)) return false;
      placements.set(p.path, p);
      if (p.children && !visit(p.children)) return false;
    }
    return true;
  }
  if (!visit(snapshot.placements)) return false;
  function scopedRanges(items: unknown, thumbnails: boolean): boolean {
    if (!Array.isArray(items)) return false;
    const seen = new Set<string>();
    for (const item of items) {
      if (!object(item) || typeof item.path !== 'string' || seen.has(item.path)) return false;
      const p = placements.get(item.path);
      if (!p || (thumbnails && p.track !== 'V1' && p.track !== 'V2') ||
          !validRanges(item.ranges, p.project[0], p.project[1])) return false;
      seen.add(item.path);
    }
    return true;
  }
  return scopedRanges(timeline.occurrences, false) && scopedRanges(timeline.thumbnails, true);
}

function layerRef(value: unknown): boolean {
  return object(value) && typeof value.occurrence === 'string' && indexPath(value.layers);
}

function geometry(value: unknown): boolean {
  return object(value) && layerRef(value.target) &&
    ['visible', 'disabled', 'inactive', 'empty', 'indeterminate'].includes(String(value.visibility));
}

function editorTarget(value: unknown): value is EditorTarget {
  return object(value) &&
    (value.kind === 'layer' && layerRef(value) ||
     value.kind === 'placement' && typeof value.occurrence === 'string' ||
     value.kind === 'track' && typeof value.scope === 'string' && typeof value.track === 'string');
}

function marqueeBounds(value: unknown, [width, height]: ViewerDocument['size']): boolean {
  return Array.isArray(value) && value.length === 4 &&
    value.every(coordinate => Number.isFinite(coordinate) && coordinate >= 0) &&
    value[0] <= value[2] && value[1] <= value[3] && value[2] <= width && value[3] <= height;
}

function editorState(value: unknown, doc: ViewerDocument): boolean {
  if (!object(value) || value.sessionId !== doc.sessionId || value.revision !== doc.revision ||
      integer(value.frame, 'editor frame') >= doc.snapshot.duration ||
      !Number.isSafeInteger(value.hudVersion) || Number(value.hudVersion) < 0 ||
      typeof value.scope !== 'string' || typeof value.dirty !== 'boolean' ||
      typeof value.conflict !== 'boolean' || typeof value.canUndo !== 'boolean' || typeof value.canRedo !== 'boolean' ||
      !Array.isArray(value.tracks) || !value.tracks.every(track => object(track) &&
        typeof track.scope === 'string' && typeof track.track === 'string' &&
        typeof track.enabled === 'boolean' && typeof track.editable === 'boolean'))
    return false;
  const { selection, selections, selectionSource, hud, marquee, hover } = value;
  return Array.isArray(selections) && selections.every(editorTarget) &&
    (selections.length === 0 ? selection === null : editorTarget(selection) && sameEditorTarget(selection, selections[0])) &&
    (selectionSource === null || object(selectionSource) && typeof selectionSource.file === 'string' &&
      indexPath(selectionSource.layers)) && (hud === null || geometry(hud)) &&
    (marquee === null || marqueeBounds(marquee, doc.size)) && (hover === null || layerRef(hover));
}

function validateEditorResult(
  command: EditorCommand, result: unknown, doc: ViewerDocument, transition?: ReopenedDocument,
) {
  if (!object(result)) throw new Error('Invalid native editor reply');
  let valid;
  switch (command.op) {
    case 'setTrackEnabled':
    case 'undo':
    case 'redo':
    case 'reload':
      valid = editorState(result.state, doc) &&
        (transition
          ? object(result.change) && result.change.fromRevision === transition.change.fromRevision &&
            result.change.toRevision === transition.change.toRevision
          : result.change === null);
      break;
    case 'layerGeometry':
      valid = geometry(result);
      break;
    case 'hitTest':
      valid = typeof result.indeterminate === 'boolean' && Array.isArray(result.hits) &&
        result.hits.every(layerRef) && (result.obstruction === undefined || layerRef(result.obstruction)) &&
        (result.fallbackPlacement === undefined || typeof result.fallbackPlacement === 'string' &&
          !result.indeterminate && result.hits.length === 0);
      break;
    case 'inspect':
      valid = result.duration === doc.snapshot.duration && Array.isArray(result.placements) &&
        result.placements.every(placement) && object(result.definitions);
      break;
    case 'render':
      valid = result.revision === doc.revision && result.frame === command.frame && Array.isArray(result.pixels);
      break;
    default:
      valid = editorState(result, doc);
  }
  if (!valid) throw new Error('Invalid native editor result for ' + command.op);
}

class EditorRequestError extends Error {
  readonly detail: { code: string; message: string };
  constructor(code: string, message: string) {
    super(message);
    this.detail = { code, message };
  }
}

function exportStatus(value: unknown): NativeExportStatus {
  if (object(value)) {
    switch (value.state) {
      case 'idle':
      case 'cancelled':
        return { state: value.state };
      case 'running':
      case 'cancelling': {
        const frames = integer(value.frames, 'export duration');
        const rendered = integer(value.rendered, 'export progress', frames);
        if (frames > 0) return { state: value.state, rendered, frames };
        break;
      }
      case 'completed': {
        const report = value.report;
        if (object(report) && typeof report.rgba_sha256 === 'string' && /^[0-9a-f]{64}$/i.test(report.rgba_sha256)) {
          return { state: 'completed', report: {
            frames: integer(report.frames, 'exported frames'),
            audio_sample_frames: integer(report.audio_sample_frames, 'exported audio samples'),
            audio_sample_rate: integer(report.audio_sample_rate, 'exported audio sample rate'),
            rgba_sha256: report.rgba_sha256,
          } };
        }
        break;
      }
      case 'failed':
        if (typeof value.error === 'string') return { state: 'failed', error: value.error };
        break;
    }
  }
  throw new Error('Invalid N4 export status');
}

/** fd 3 carries ordered, tightly packed RGBA8; a read can span multiple OS chunks. */
class PayloadReader {
  private readonly chunks: AsyncIterator<Buffer>;
  private carry = Buffer.alloc(0) as Buffer;
  constructor(stream: Readable) { this.chunks = stream[Symbol.asyncIterator](); }

  async read(bytes: number): Promise<Uint8Array> {
    const image = new Uint8Array(bytes);
    let offset = 0;
    while (offset < bytes) {
      if (!this.carry.length) {
        const next = await this.chunks.next();
        if (next.done) throw new Error('Truncated native picture');
        this.carry = next.value;
      }
      const count = Math.min(bytes - offset, this.carry.length);
      image.set(this.carry.subarray(0, count), offset);
      this.carry = this.carry.subarray(count);
      offset += count;
    }
    return image;
  }
}

export class NativeSession implements ViewerBridge {
  document!: ViewerDocument;
  private readonly child: ChildProcess;
  private readonly requests = new Map<number, Waiter>();
  private serial = 0;
  private stopped?: Error;
  private closing?: Promise<void>;
  readonly closed: Promise<void>;
  private changeListeners = new Set<() => void>();
  private missedChange?: number;

  private constructor(executable: string, project: string, signal?: AbortSignal, editor = false) {
    this.child = spawn(executable, editor ? ['--editor', project] : [project],
      { stdio: ['pipe', 'pipe', 'pipe', 'pipe'], signal });
    let diagnostic = '';
    this.child.stderr!.setEncoding('utf8').on('data', data => { diagnostic = (diagnostic + data).slice(-4096); });
    this.child.on('error', error => this.fail(error));
    this.child.stdin!.on('error', error => this.fail(error));
    this.closed = new Promise(done => this.child.once('close', (code, signal) => {
      this.fail(new Error('N4 process closed (' + (signal ?? code) + ') ' + diagnostic.trim()));
      done();
    }));
    void this.receive().catch(error => { this.fail(error); this.child.kill(); });
  }

  static async open(executable: string, project: string, signal?: AbortSignal, options: { editor?: boolean } = {}): Promise<NativeSession> {
    const session = new NativeSession(executable, project, signal, options.editor);
    try {
      const { fields } = await session.request('inspect');
      session.document = document(fields);
      if (Boolean(session.document.editable) !== Boolean(options.editor)) throw new Error('Unexpected native editor grant');
      return session;
    } catch (error) {
      session.terminate();
      throw error;
    }
  }

  private fail(error: Error) {
    this.stopped ??= error;
    this.changeListeners.clear();
    for (const waiter of this.requests.values()) waiter.reject(this.stopped);
    this.requests.clear();
  }

  private async receive() {
    const payload = new PayloadReader(this.child.stdio[3] as Readable);
    for await (const line of createInterface({ input: this.child.stdout!, crlfDelay: Infinity })) {
      const fields: unknown = JSON.parse(line);
      if (!object(fields)) throw new Error('Invalid N4 response');
      if (fields.event === 'inputsChanged' && fields.id === undefined) {
        const revision = integer(fields.revision, 'change notice revision');
        if (this.closing || this.stopped || (this.document && revision < this.document.revision)) continue;
        if (!this.changeListeners.size) this.missedChange = revision;
        else for (const listener of this.changeListeners) listener();
        continue;
      }
      if (typeof fields.ok !== 'boolean') throw new Error('Invalid N4 response');
      const id = integer(fields.id, 'request id'), pending = this.requests.get(id);
      if (!pending) throw new Error('Unknown N4 response id');
      if (fields.ok === false) {
        const editorError = fields.editorError;
        pending.reject(object(editorError) && typeof editorError.code === 'string' &&
          typeof editorError.message === 'string' ? new EditorRequestError(editorError.code, editorError.message)
          : new Error(String(fields.error)));
      } else {
        let rgba: Uint8Array | undefined;
        if (fields.bytes !== undefined && fields.bytes !== 0) {
          const width = integer(fields.width, 'width', 8192), height = integer(fields.height, 'height', 8192);
          if (!width || !height || fields.bytes !== width * height * 4) throw new Error('Invalid RGBA payload size');
          rgba = await payload.read(fields.bytes as number);
        }
        pending.resolve({ fields, rgba });
      }
      this.requests.delete(id);
    }
  }

  private request(op: string, args: Json = {}): Promise<Reply> {
    if (this.stopped || (this.closing && op !== 'close')) return Promise.reject(this.stopped ?? new Error('Session closing'));
    if (this.requests.size >= 16) return Promise.reject(new Error('N4 host command queue full'));
    const id = ++this.serial;
    const revision = op === 'inspect' || op === 'close' ? undefined : this.document?.revision;
    const line = JSON.stringify({ id, op, revision, ...args }) + '\n';
    return new Promise((resolve, reject) => {
      this.requests.set(id, { resolve, reject });
      (this.child.stdin as Writable).write(line);
    });
  }

  async inspect() { return document((await this.request('inspect')).fields); }
  /** Native work counters for a headless benchmark of the accepted revision. */
  async stats() {
    const { fields } = await this.request('stats');
    if (fields.revision !== this.document.revision || !object(fields.frames))
      throw new Error('Invalid N4 session stats');
    const frames = fields.frames;
    return { revision: fields.revision, audioPreparations: integer(fields.audioPreparations, 'audio preparations'),
      frames: {
        hits: integer(frames.hits, 'frame cache hits'), misses: integer(frames.misses, 'frame cache misses'),
        inherited: integer(frames.inherited, 'inherited frames'), entries: integer(frames.entries, 'resident frames'),
        bytes: integer(frames.bytes, 'resident frame bytes'), budgetBytes: integer(frames.budgetBytes, 'frame budget'),
      } };
  }
  onProjectChange(listener: () => void) {
    if (this.stopped || this.closing) throw this.stopped ?? new Error('Session closing');
    this.changeListeners.add(listener);
    const missed = this.missedChange;
    this.missedChange = undefined;
    if (missed !== undefined && missed >= this.document.revision) listener();
    return () => { this.changeListeners.delete(listener); };
  }
  private transition(reply: Reply, previous: ViewerDocument): ReopenedDocument {
    const accepted = document(reply.fields);
    const change = reply.fields.change;
    if (previous.editable !== accepted.editable || !isRevisionChange(change, accepted.snapshot) ||
        !continuesSession(previous, accepted, change))
      throw new Error('Mismatched project revision change');
    const parked = picture(reply, integer(reply.fields.generation, 'generation'));
    if (parked.revision !== accepted.revision || parked.frame >= accepted.snapshot.duration ||
        parked.width !== accepted.size[0] || parked.height !== accepted.size[1])
      throw new Error('Mismatched parked picture on reopen');
    return { ...accepted, parked, change };
  }

  async reopen(frame?: number) {
    const previous = this.document;
    const { parked, change, ...accepted } = this.transition(await this.request('reopen', { frame }), previous);
    this.document = accepted;
    return { ...accepted, parked, change };
  }

  async editor<C extends EditorCommand>(command: C): Promise<EditorReply<C>> {
    if (!this.document.editable) throw new Error('Editor session is unavailable');
    const previous = this.document;
    try {
      const reply = await this.request('editor', { command });
      const transition = reply.fields.change === undefined ? undefined : this.transition(reply, previous);
      if (reply.rgba && !transition) throw new Error('Unexpected picture in native editor reply');
      validateEditorResult(command, reply.fields.result, transition ?? this.document, transition);
      if (transition) {
        const { parked: _, change: __, ...accepted } = transition;
        this.document = accepted;
      }
      return { result: reply.fields.result as EditorResult<C>, ...(transition && { transition }) };
    } catch (error) {
      if (error instanceof EditorRequestError) return { error: error.detail };
      throw error;
    }
  }

  async hud(revision: number, frame: number): Promise<HudSample> {
    const doc = this.document;
    if (!doc.editable) throw new Error('Editor HUD is unavailable');
    const reply = await this.request('hud', { revision, frame });
    const hudVersion = integer(reply.fields.hudVersion, 'HUD version');
    if (reply.fields.empty === true && !reply.rgba) return { hudVersion, tile: null };
    const tile = picture(reply, integer(reply.fields.generation, 'generation'), frame);
    const x = integer(reply.fields.x, 'HUD x'), y = integer(reply.fields.y, 'HUD y');
    if (tile.revision !== revision || tile.width + x > doc.size[0] || tile.height + y > doc.size[1])
      throw new Error('Host returned a mismatched HUD tile');
    return { hudVersion, tile: { ...tile, x, y } };
  }

  async seek(frame: number, generation = 1, previewOnly = false) {
    return picture(await this.request('seek', { frame, generation, ...(previewOnly && { previewOnly }) }), generation, frame);
  }
  async thumbnail(frame: number, generation = 1, placement?: string) {
    return picture(await this.request('thumbnail', { frame, generation, placement }), generation, frame);
  }
  async play(frame: number, generation: number) { await this.request('play', { frame, generation }); }
  async pause(generation: number) { await this.request('pause', { generation }); }
  async next(generation: number): Promise<PlaybackSample> {
    const reply = await this.request('next', { generation });
    return reply.rgba ? { picture: picture(reply, generation), playing: true }
      : { picture: null, playing: reply.fields.playing === true };
  }
  async presented(frame: number, generation: number) { await this.request('presented', { frame, generation }); }

  async startExport(path: string, revision = this.document.revision) {
    return exportStatus((await this.request('export', { path, revision })).fields.status);
  }

  async exportStatus() { return exportStatus((await this.request('exportstatus')).fields.status); }
  async cancelExport() { return exportStatus((await this.request('cancelexport')).fields.status); }

  terminate() {
    this.fail(new Error('N4 session disposed'));
    this.child.kill();
  }

  close(): Promise<void> {
    this.changeListeners.clear();
    return this.closing ??= (async () => {
      const deadline = setTimeout(() => this.terminate(), 3000);
      try { if (!this.stopped) await this.request('close'); }
      finally { this.child.stdin?.end(); await this.closed; clearTimeout(deadline); }
    })();
  }
}
