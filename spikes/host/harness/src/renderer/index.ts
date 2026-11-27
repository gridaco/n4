import type { EditorState, ViewerBridge } from '@n4/host-kit/types';
import { mountViewer } from '@n4/host-kit/viewer';
import { formatElapsedTime } from '@n4/host-kit/timeline';
import { createIcons, Download, Pause, Play, Redo2, RotateCw, Save, Undo2 } from 'lucide';
import { mountTimelineDivider } from './timeline-divider.ts';

declare global {
  interface Window {
    n4Viewer: ViewerBridge;
    openN4Export(): Promise<void>;
  }
}
const bridge = window.n4Viewer;
const params = new URL(location.href).searchParams;
if (params.get('insetTitleBar') === '1') {
  document.querySelector<HTMLElement>('[data-role=viewer-toolbar]')!.dataset.windowChrome = 'inset';
}
const play = document.querySelector<HTMLButtonElement>('[data-role=play]')!;
const playLabel = play.querySelector('span')!;
const reopen = document.querySelector<HTMLButtonElement>('[data-role=reopen]')!;
const exportButton = document.querySelector<HTMLButtonElement>('[data-role=export]')!;
const status = document.querySelector<HTMLElement>('[role=status]')!;
const title = document.querySelector<HTMLElement>('h1')!;
const badge = document.querySelector<HTMLOutputElement>('output')!;
const previewCanvas = document.querySelector<HTMLCanvasElement>('[data-role=preview]')!;
const hudCanvas = document.querySelector<HTMLCanvasElement>('[data-role=preview-hud]')!;
const editorActions = document.querySelector<HTMLElement>('[data-role=editor-actions]')!;
const undo = document.querySelector<HTMLButtonElement>('[data-role=undo]')!;
const redo = document.querySelector<HTMLButtonElement>('[data-role=redo]')!;
const save = document.querySelector<HTMLButtonElement>('[data-role=save]')!;
const timelineRoot = document.querySelector<HTMLElement>('[data-role=timeline-root]')!;
const divider = document.querySelector<HTMLElement>('[data-role=timeline-divider]')!;
const disposeDivider = mountTimelineDivider(document.body, divider, previewCanvas.parentElement!, timelineRoot);
window.addEventListener('pagehide', disposeDivider, { once: true });

function report(message: string, level = 'info') {
  if (status.textContent !== message) status.textContent = message;
  if (status.title !== message) status.title = message;
  if (status.dataset.level !== level) status.dataset.level = level;
}
function diagnostic(error: unknown) {
  report(error instanceof Error ? error.message : String(error), 'error');
}

async function mount() {
  createIcons({ icons: { Download, Pause, Play, Redo2, RotateCw, Save, Undo2 }, attrs: { 'aria-hidden': 'true' } });
  let refreshing = false;
  let editorState: EditorState | undefined;
  function updateEditorControls() {
    editorActions.hidden = !editorState;
    previewCanvas.toggleAttribute('data-editable', !!editorState);
    undo.disabled = refreshing || !editorState?.canUndo || editorState.conflict;
    redo.disabled = refreshing || !editorState?.canRedo || editorState.conflict;
    save.disabled = refreshing || !editorState?.dirty || editorState.conflict;
  }
  function setRefreshing(active: boolean) {
    refreshing = active;
    play.disabled = active;
    reopen.disabled = active;
    exportButton.disabled = active;
    updateEditorControls();
  }
  const viewer = await mountViewer(bridge, {
    preview: previewCanvas,
    hud: hudCanvas,
    timeline: timelineRoot,
    frame: Number(params.get('frame')) || 0,
    onPicture(frame, _running, doc) { badge.textContent = formatElapsedTime(frame, doc.snapshot.fps); },
    onPlayback(active) {
      playLabel.textContent = active ? 'Pause' : 'Play';
    },
    onRefreshing: setRefreshing,
    onEditorState(state) {
      editorState = state;
      updateEditorControls();
    },
    onRefresh(doc) {
      title.textContent = doc.snapshot.name;
      report('');
    },
    onError: diagnostic,
  });
  title.textContent = viewer.document.snapshot.name;
  setRefreshing(refreshing);
  play.onclick = () => { if (viewer.playing) viewer.pause(); else void viewer.play(); };
  if (viewer.editor) {
    undo.onclick = () => { void viewer.editor!.undo().catch(diagnostic); };
    redo.onclick = () => { void viewer.editor!.redo().catch(diagnostic); };
    save.onclick = () => { void viewer.editor!.save().then(() => report('Saved'), diagnostic); };
  }
  document.addEventListener('keydown', event => {
    if (viewer.editor && (event.metaKey || event.ctrlKey) && !event.repeat) {
      const key = event.key.toLowerCase();
      const action = key === 's' ? save : key === 'y' || (key === 'z' && event.shiftKey) ? redo : key === 'z' ? undo : undefined;
      if (action) {
        event.preventDefault();
        action.click();
        return;
      }
    }
    if (event.code === 'Space' && !event.repeat && !(event.target instanceof HTMLButtonElement)) {
      event.preventDefault();
      play.click();
    }
  });
  document.addEventListener('visibilitychange', () => { if (document.hidden) viewer.pause(); });
  reopen.onclick = () => { void viewer.reopen().catch(diagnostic); };
  exportButton.onclick = async () => {
    viewer.pause();
    try { await window.openN4Export(); } catch (error) { diagnostic(error); }
  };
  window.addEventListener('pagehide', () => viewer.dispose(), { once: true });
}
void mount().catch(diagnostic);
