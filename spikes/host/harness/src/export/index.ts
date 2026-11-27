import { formatElapsedTime } from '@n4/host-kit/timeline';
import type { ExportStatus } from '@n4/host-kit/types';
import { createIcons, Download, Film } from 'lucide';
import type { ExportDialogBridge, ExportProject } from './types.ts';

declare global { interface Window { n4Export: ExportDialogBridge } }

const bridge = window.n4Export;
const projectName = document.querySelector<HTMLElement>('#export-project')!;
const size = document.querySelector<HTMLElement>('[data-field=size]')!;
const fps = document.querySelector<HTMLElement>('[data-field=fps]')!;
const duration = document.querySelector<HTMLElement>('[data-field=duration]')!;
const status = document.querySelector<HTMLElement>('[role=status]')!;
const meter = document.querySelector<HTMLElement>('#export-meter')!;
const progress = document.querySelector<HTMLProgressElement>('[data-role=export-progress]')!;
const percent = document.querySelector<HTMLElement>('[data-role=export-percent]')!;
const start = document.querySelector<HTMLButtonElement>('[data-role=start-export]')!;
const cancel = document.querySelector<HTMLButtonElement>('[data-role=cancel-export]')!;
const close = document.querySelector<HTMLButtonElement>('[data-role=close-export]')!;

let active = true;
let closing = false;
let cancelling = false;

function report(text: string, level = 'info', detail = text) {
  if (status.textContent !== text) status.textContent = text;
  status.dataset.level = level;
  status.title = detail;
}

function reportError(error: unknown) {
  report(error instanceof Error ? error.message : String(error), 'error');
}

function showProject(project: ExportProject) {
  projectName.textContent = project.name;
  projectName.title = project.name;
  size.textContent = project.size.join(' × ');
  fps.textContent = new Intl.NumberFormat(undefined, { maximumFractionDigits: 3 })
    .format(project.fps[0] / project.fps[1]) + ' fps';
  duration.textContent = formatElapsedTime(project.duration, project.fps);
}

function showProgress(job: Extract<ExportStatus, { rendered: number }>) {
  meter.hidden = progress.hidden = false;
  cancel.hidden = false;
  close.hidden = true;
  cancel.disabled = cancelling || job.state === 'cancelling';
  progress.max = job.frames;
  progress.value = job.rendered;
  percent.textContent = Math.floor(job.rendered / job.frames * 100) + '%';
  if (!cancel.disabled && (document.activeElement === document.body || document.activeElement === start)) {
    cancel.focus({ preventScroll: true });
  }
  if (cancel.disabled) report('Cancelling export…');
  else report(job.rendered < job.frames ? 'Exporting MP4…' : 'Saving MP4…');
}

async function exportVideo() {
  start.disabled = true;
  report('');
  try {
    const project = await bridge.inspect();
    showProject(project);
    let job = await bridge.startExport(project.revision);
    if (!job) return;
    while (active && (job.state === 'running' || job.state === 'cancelling')) {
      showProgress(job);
      await new Promise(resolve => setTimeout(resolve, 200));
      if (!active) return;
      job = await bridge.exportStatus();
    }
    if (!active) return;
    switch (job.state) {
      case 'completed':
        report('Exported ' + job.path.split(/[\\/]/).at(-1), 'success', job.path);
        break;
      case 'cancelled': report('Export cancelled'); break;
      case 'failed': reportError(job.error); break;
      case 'idle': reportError('The export job ended before saving a video.'); break;
    }
  } catch (error) {
    if (active) reportError(error);
  } finally {
    meter.hidden = progress.hidden = cancel.hidden = true;
    cancelling = cancel.disabled = close.hidden = start.disabled = false;
    if (document.activeElement === document.body || document.activeElement === cancel) {
      start.focus({ preventScroll: true });
    }
  }
}

async function cancelVideo() {
  cancel.disabled = cancelling = true;
  report('Cancelling export…');
  try {
    await bridge.cancelExport();
  } catch (error) {
    if (active) reportError(error);
    cancel.disabled = cancelling = false;
  }
}

async function closeDialog() {
  if (closing) return;
  closing = true;
  try {
    await bridge.requestClose();
  } catch (error) {
    if (active) reportError(error);
  } finally {
    closing = false;
  }
}

async function mount() {
  createIcons({ icons: { Download, Film }, attrs: { 'aria-hidden': 'true' } });
  close.onclick = closeDialog;
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape') void closeDialog();
  });
  window.addEventListener('pagehide', () => { active = false; }, { once: true });
  const project = await bridge.inspect();
  showProject(project);
  start.onclick = () => void exportVideo();
  cancel.onclick = cancelVideo;
  start.disabled = false;
  start.focus();
}
void mount().catch(reportError);
