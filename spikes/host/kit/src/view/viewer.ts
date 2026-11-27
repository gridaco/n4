// Browser entry for a parent that wants both N4 surfaces in its own layout.
import type { EditorCommand, EditorState, ReopenedDocument, RevisionChange, ViewerBridge, ViewerDocument } from '../client/types.ts';
import { continuesSession, LiveReload } from '../client/live-reload.ts';
import { TimelineController, type Placement } from '../timeline/index.ts';
import { mountPreview, imageData, type PreviewEvents } from './preview.ts';
import { mountTimeline } from './timeline.ts';
import { mountEditorHud } from './editor-hud.ts';

export interface ViewerOptions {
  preview: HTMLCanvasElement;
  hud?: HTMLCanvasElement;
  timeline: HTMLElement;
  frame?: number;
  onPicture?: (frame: number, playing: boolean, document: ViewerDocument) => void;
  onPlayback?: (playing: boolean) => void;
  onRefreshing?: (refreshing: boolean) => void;
  onRefresh?: (document: ViewerDocument, change: RevisionChange) => void;
  onEditorState?: (state: EditorState) => void;
  onError: PreviewEvents['error'];
}

export async function mountViewer(bridge: ViewerBridge, options: ViewerOptions) {
  // A save may arrive between opening the native session and mounting its surfaces.
  let changedWhileOpening = false;
  let onChange = () => { changedWhileOpening = true; };
  const unsubscribe = bridge.onProjectChange?.(() => onChange());
  try {
    const initial = await bridge.inspect();
    let editor: EditorState | undefined;
    if (initial.editable && bridge.editor) {
      const response = await bridge.editor({op: 'getState'});
      if ('error' in response) throw new Error(response.error.message);
      editor = response.result;
    }
    const { viewer, externalChange } = mountDocument(bridge, options, initial, editor, unsubscribe);
    onChange = externalChange;
    if (changedWhileOpening) externalChange();
    return viewer;
  } catch (error) {
    unsubscribe?.();
    throw error;
  }
}

type ViewerEditorCommand = Extract<EditorCommand, {
  op: 'select' | 'enterScope' | 'setTrackEnabled' | 'undo' | 'redo' | 'save' | 'reload' |
    'beginMarquee' | 'updateMarquee' | 'finishMarquee' | 'cancelMarquee' | 'hoverAt' | 'clearHover'
}>;

function mountDocument(
  bridge: ViewerBridge, options: ViewerOptions, initial: ViewerDocument,
  startingEditor?: EditorState, unsubscribe?: () => void,
) {
  let doc = initial;
  let editor = startingEditor;
  let publishedEditor: EditorState | undefined;
  let playing = false;
  let disposed = false;
  let editing = false;
  let deferredSeek: number | undefined;
  let displayedFrame: number | undefined;
  let editorQueue = Promise.resolve();
  const hud = editor && options.hud && bridge.hud ? mountEditorHud(options.hud, bridge, options.onError) : undefined;
  const abort = new AbortController();
  const reload = new LiveReload(reopenDocument, {
    onError: options.onError,
    onBusy: updateBusy,
  });
  const timeline = new TimelineController(
    doc.snapshot,
    intent => requestSeek(intent.frame, intent.phase),
    frame => {
      if (frame === null) preview.clearSkim();
      else preview.skim(frame);
    },
    editor && {
      select(target) {
        const revision = doc.revision;
        void runEditor(() => ({op: 'select', revision, target})).catch(options.onError);
      },
      enterScope(scope) {
        const revision = doc.revision;
        void runEditor(() => ({op: 'enterScope', revision, scope})).catch(options.onError);
      },
      setTrackEnabled(scope, track, enabled) {
        void runEditor(revision => ({op: 'setTrackEnabled', revision, scope, track, enabled}), true).catch(options.onError);
      },
    },
  );
  const preview = mountPreview(options.preview, bridge, doc, {
    picture(frame, running) {
      displayedFrame = frame;
      if (running) timeline.setPlaybackFrame(frame);
      refreshHud();
      options.onPicture?.(frame, running, doc);
    },
    playback(active, startFrame) {
      playing = active;
      if (!disposed) view.setPlaying(active);
      if (startFrame !== undefined) timeline.setPlaybackFrame(startFrame);
      timeline.setSkimmingEnabled(!active && !reload.busy && !editing);
      refreshHud();
      options.onPlayback?.(active);
    },
    seeking(active) {
      view.setSeeking(active);
    },
    error(error) {
      // Accepting a reload can supersede a seek from the previous revision.
      if (!reload.busy && !editing && !disposed) options.onError(error);
    },
  });
  const media = ({ peaks, revision }: ViewerDocument) => ({
    peaks,
    thumbnail: async (placement: Placement, frame: number) => {
      const picture = await bridge.thumbnail(frame, 0, placement.path);
      return picture?.revision === revision ? imageData(picture) : null;
    },
  });
  const view = mountTimeline(options.timeline, timeline, media(doc));

  function refreshHud() {
    if (!hud) return;
    const visible = editor && (editor.hover || editor.marquee || editor.selections.some(target => target.kind === 'layer'));
    if (!editor || playing || displayedFrame === undefined || !visible) hud.clear();
    else hud.draw(doc, displayedFrame, editor.hudVersion);
  }

  function acceptState(state: EditorState) {
    if (publishedEditor?.sessionId === state.sessionId && publishedEditor.hudVersion >= state.hudVersion) return;
    editor = state;
    publishedEditor = state;
    timeline.setEditorState(state);
    options.onEditorState?.(state);
    refreshHud();
  }

  function updateBusy() {
    const busy = reload.busy || editing;
    view.setRefreshing(busy);
    if (busy) preview.pause();
    timeline.setSkimmingEnabled(!busy && !playing);
    if (!busy && deferredSeek !== undefined) {
      const frame = Math.min(deferredSeek, doc.snapshot.duration - 1);
      deferredSeek = undefined;
      timeline.setPlaybackFrame(frame);
      preview.seek(frame, 'exact');
    }
    options.onRefreshing?.(busy);
  }

  function requestSeek(frame: number, phase: 'scrub' | 'exact') {
    if (reload.busy || editing) deferredSeek = frame;
    else preview.seek(frame, phase);
  }

  function seek(frame: number) {
    if (disposed) return;
    if (!Number.isSafeInteger(frame)) throw new RangeError('Expected an integer project frame');
    timeline.setPlaybackFrame(Math.max(0, Math.min(doc.snapshot.duration - 1, frame)));
    requestSeek(timeline.cursor, 'exact');
  }

  function installDocument(result: ReopenedDocument, state?: EditorState) {
    const { parked, change, ...accepted } = result;
    const transition = continuesSession(doc, accepted, change) ? change : undefined;
    doc = accepted;
    timeline.replace(doc.snapshot, transition);
    if (state) acceptState(state);
    view.setMedia(media(doc), transition?.timeline);
    if (!state && deferredSeek === undefined && timeline.cursor !== parked.frame) deferredSeek = timeline.cursor;
    preview.replace(doc, parked);
    options.onRefresh?.(doc, change);
  }

  function runEditor(command: (revision: number) => ViewerEditorCommand | undefined, changesPicture = false) {
    const next = editorQueue.then(async () => {
      if (disposed || !editor || !bridge.editor) return;
      const request = command(doc.revision);
      if (!request) return;
      if (changesPicture) {
        editing = true;
        updateBusy();
      }
      try {
        const reply = await bridge.editor(request);
        if (disposed) return;
        if ('error' in reply) {
          if (reply.error.code === 'diskConflict') {
            const current = await bridge.editor({op: 'getState'});
            if (!disposed && 'result' in current) acceptState(current.result);
          }
          throw new Error(reply.error.message);
        }
        const state = 'state' in reply.result ? reply.result.state : reply.result;
        if (reply.transition) installDocument(reply.transition, state);
        else acceptState(state);
      } finally {
        if (changesPicture && !disposed) {
          editing = false;
          updateBusy();
        }
      }
    });
    editorQueue = next.catch(() => {});
    return next;
  }

  async function reopenDocument() {
    try {
      if (editor) await runEditor(revision => ({op: 'reload', revision}));
      else {
        const result = await bridge.reopen(timeline.cursor);
        if (!disposed) installDocument(result);
      }
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      throw new Error('Showing the last valid revision. ' + reason, { cause: error });
    }
    return doc;
  }

  if (editor) {
    acceptState(editor);
    const surface = options.preview;
    const { signal } = abort;
    let gesture: { id: number; revision: number } | undefined;
    let latest: ViewerEditorCommand | undefined;
    let scheduled = 0;
    let pending = false;
    function pointer(event: PointerEvent) {
      const { width, height, left, top } = surface.getBoundingClientRect();
      const [w, h] = doc.size;
      const scale = Math.min(width / w, height / h);
      if (!(scale > 0)) return undefined;
      // The preview and HUD share the centered, object-contain picture rectangle.
      const x = (event.clientX - left - (width - w * scale) / 2) / scale;
      const y = (event.clientY - top - (height - h * scale) / 2) / scale;
      return { point: [x, y] as [number, number], scale, inside: x >= 0 && x < w && y >= 0 && y < h };
    }
    function send(command: ViewerEditorCommand) {
      return runEditor(revision => revision === command.revision ? command : undefined).catch(options.onError);
    }
    function schedulePointer(command: ViewerEditorCommand) {
      latest = command;
      schedule();
    }
    function schedule() {
      if (!latest || scheduled || pending || signal.aborted) return;
      scheduled = requestAnimationFrame(() => {
        scheduled = 0;
        if (!latest) return;
        const command = latest;
        latest = undefined;
        pending = true;
        void send(command).finally(() => {
          pending = false;
          schedule();
        });
      });
    }
    function resetPointer() {
      latest = undefined;
      cancelAnimationFrame(scheduled);
      scheduled = 0;
    }
    signal.addEventListener('abort', resetPointer, { once: true });
    surface.addEventListener('pointerdown', event => {
      if (event.button !== 0 || gesture || displayedFrame === undefined || reload.busy || editing) return;
      const input = pointer(event);
      if (!input?.inside) return;
      event.preventDefault();
      resetPointer();
      const frame = displayedFrame, revision = doc.revision;
      if (playing) preview.pause();
      gesture = { id: event.pointerId, revision };
      surface.setPointerCapture(event.pointerId);
      void send({op: 'beginMarquee', revision, frame, point: input.point, threshold: 4 / input.scale});
    }, { signal });
    surface.addEventListener('pointermove', event => {
      const input = pointer(event);
      if (!input) return;
      if (gesture) {
        if (gesture.id === event.pointerId)
          schedulePointer({op: 'updateMarquee', revision: gesture.revision, point: input.point});
      } else if (event.pointerType !== 'touch' && !event.buttons && !playing && !reload.busy && !editing && displayedFrame !== undefined) {
        const revision = doc.revision;
        schedulePointer(input.inside
          ? {op: 'hoverAt', revision, frame: displayedFrame, point: input.point}
          : {op: 'clearHover', revision});
      }
    }, { signal });
    surface.addEventListener('pointerleave', () => {
      if (!gesture) schedulePointer({op: 'clearHover', revision: doc.revision});
    }, { signal });
    function cancel() {
      if (!gesture) return;
      const { revision, id } = gesture;
      gesture = undefined;
      resetPointer();
      if (surface.hasPointerCapture(id)) surface.releasePointerCapture(id);
      void send({op: 'cancelMarquee', revision});
    }
    surface.addEventListener('pointerup', event => {
      if (gesture?.id !== event.pointerId) return;
      const input = pointer(event);
      if (!input) return cancel();
      const { revision } = gesture;
      gesture = undefined;
      resetPointer();
      void send({op: 'finishMarquee', revision, point: input.point});
    }, { signal });
    function lost(event: PointerEvent) {
      if (gesture?.id === event.pointerId) cancel();
    }
    surface.addEventListener('pointercancel', lost, { signal });
    surface.addEventListener('lostpointercapture', lost, { signal });
    window.addEventListener('blur', () => {
      if (gesture) cancel();
      else {
        resetPointer();
        void send({op: 'clearHover', revision: doc.revision});
      }
    }, { signal });
    window.addEventListener('keydown', event => {
      if (event.key !== 'Escape' || !gesture) return;
      event.preventDefault();
      event.stopPropagation();
      cancel();
    }, { capture: true, signal });
  }

  seek(options.frame ?? 0);
  const viewer = {
    get document() { return doc; },
    get playing() { return playing; },
    timeline,
    seek,
    play() {
      return reload.busy || editing ? Promise.resolve() : preview.play(timeline.cursor, timeline.skimFrame ?? undefined);
    },
    pause() {
      preview.pause();
      timeline.clearSkim();
    },
    reopen: () => reload.refresh(),
    editor: editor && {
      undo: () => runEditor(revision => ({op: 'undo', revision}), true),
      redo: () => runEditor(revision => ({op: 'redo', revision}), true),
      save: () => runEditor(revision => ({op: 'save', revision})),
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      abort.abort();
      hud?.clear();
      unsubscribe?.();
      reload.dispose();
      preview.dispose();
      view.dispose();
    },
  };
  return { viewer, externalChange: () => reload.changed() };
}
