import { bodyTop, headerHeight, rulerHeight } from '../timeline/index.ts';
import { formatElapsedTime } from '../timeline/time.ts';
import type { Clip, FrameRange, Row, TimelineAction, TimelineController, TimelineLayout } from '../timeline/index.ts';
import type { TimelineChange } from '../client/types.ts';

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, role?: string) {
  const node = document.createElement(tag);
  node.className = 'n4-timeline-' + className;
  if (role) node.dataset.role = role;
  return node;
}

function rowLabel(row: Row) {
  if (row.kind !== 'lane') return row.label;
  if (row.label === 'CAM') return 'Camera cuts';
  const track = /^([VA])(\d+)$/.exec(row.label);
  if (!track) return row.label;
  return (track[1] === 'V' ? 'Video ' : 'Audio ') + track[2];
}

export function createTimelineControls(controller: TimelineController) {
  const root = element('div', 'surface');
  const header = element('div', 'header');
  const breadcrumb = element('div', 'breadcrumb');
  const actions = element('div', 'actions');
  const sidebar = element('div', 'sidebar', 'timeline-sidebar');
  const time = element('output', 'time', 'timeline-time');
  const labels = element('div', 'labels');
  const trackControls = element('div', 'track-controls');
  const trackToggle = element('button', 'track-toggle', 'timeline-track-toggle');
  trackToggle.type = 'button';
  trackToggle.hidden = true;
  const plot = element('div', 'plot');
  const canvas = element('canvas', 'canvas', 'timeline');
  const playhead = element('div', 'playhead-wrap');
  const skimmer = element('div', 'skimmer', 'skimmer');
  const handle = element('div', 'playhead', 'playhead');
  const rows = new Map<string, HTMLButtonElement>();
  let changeCue: HTMLDivElement | undefined;

  const up = element('button', 'action');
  up.textContent = '‹';
  up.setAttribute('aria-label', 'Back to parent timeline');
  up.title = 'Back to parent timeline';
  function bindAction(button: HTMLButtonElement, action: TimelineAction) {
    button.type = 'button';
    button.dataset.timelineAction = action;
    button.onclick = () => controller.navigate(action);
  }
  bindAction(up, 'up');
  const buttons = new Map<TimelineAction, HTMLButtonElement>();
  for (const [action, name] of [
    ['nextUse', 'Next use of this sequence'],
    ['wider', 'Zoom out'],
    ['closer', 'Zoom in'],
    ['fit', 'Fit timeline'],
  ] as const) {
    const button = element('button', 'action');
    button.setAttribute('aria-label', name);
    button.title = name;
    bindAction(button, action);
    buttons.set(action, button);
    actions.append(button);
  }
  time.setAttribute('aria-label', 'Project time');
  time.setAttribute('aria-live', 'off');
  canvas.tabIndex = 0;
  canvas.setAttribute('role', 'slider');
  canvas.setAttribute('aria-label', 'Timeline playhead');
  canvas.setAttribute('aria-orientation', 'horizontal');
  canvas.setAttribute(
    'aria-description',
    'Drag the ruler to scrub. Double-click a clip to open it. Left and right arrows step a frame.',
  );
  playhead.setAttribute('aria-hidden', 'true');
  skimmer.hidden = true;
  skimmer.setAttribute('aria-hidden', 'true');
  playhead.append(handle);
  plot.append(canvas, skimmer, playhead);
  trackControls.append(trackToggle);
  sidebar.append(time, labels, trackControls);
  header.append(up, breadcrumb, actions);
  root.append(header, sidebar, plot);

  function positionCue(cue: HTMLElement, view: TimelineLayout) {
    cue.replaceChildren();
    function add(ranges: FrameRange[], top: number, bottom: number, clip?: Clip) {
      if (bottom <= top) return;
      for (const range of ranges) {
        const bounds = controller.markBounds(range, clip);
        if (!bounds) continue;
        const ring = element('div', 'change-ring');
        ring.style.left = (bounds[0] - view.gutter) + 'px';
        ring.style.width = (bounds[1] - bounds[0]) + 'px';
        ring.style.top = (top - headerHeight) + 'px';
        ring.style.height = (bottom - top) + 'px';
        cue.append(ring);
      }
    }
    add(view.rulerChanges, bodyTop - 6, bodyTop);
    for (const row of view.rows) {
      for (const clip of row.clips) {
        add(clip.changedRanges, Math.max(bodyTop, row.top + 3), row.top + row.height - 3, clip);
      }
    }
  }

  function flashChanges(change?: TimelineChange) {
    changeCue?.remove();
    changeCue = undefined;
    if (!change || (change.kind === 'known' && !change.occurrences.length)) return;
    const cue = element('div', 'change-cue');
    cue.setAttribute('aria-hidden', 'true');
    positionCue(cue, controller.layout());
    changeCue = cue;
    cue.addEventListener('animationend', () => {
      cue.remove();
      if (changeCue === cue) changeCue = undefined;
    }, { once: true });
    plot.insertBefore(cue, skimmer);
  }

  function layout(view: TimelineLayout) {
    if (changeCue) positionCue(changeCue, view);
    root.style.setProperty('--n4-timeline-gutter', view.gutter + 'px');
    root.style.setProperty('--n4-timeline-header', headerHeight + 'px');
    root.style.setProperty('--n4-timeline-ruler', rulerHeight + 'px');
    up.hidden = !view.scopePath;
    breadcrumb.textContent = view.breadcrumb;
    breadcrumb.title = view.breadcrumb;
    for (const [action, button] of buttons) {
      const item = view.buttons.find((candidate) => candidate.action === action);
      button.hidden = !item;
      if (!item) continue;
      button.textContent = item.label;
      button.style.width = item.width + 'px';
    }
    const focused = document.activeElement;
    const editableLane = view.rows.find(row => row.track?.editable);
    if (editableLane?.track) {
      const { enabled, track } = editableLane.track;
      trackToggle.hidden = false;
      trackToggle.dataset.track = track;
      trackToggle.style.top = editableLane.top - bodyTop + (editableLane.height - 24) / 2 + 'px';
      trackToggle.textContent = enabled ? 'On' : 'Off';
      trackToggle.title = 'Toggle ' + rowLabel(editableLane);
      trackToggle.setAttribute('aria-label', trackToggle.title);
      trackToggle.setAttribute('aria-pressed', String(enabled));
      trackToggle.onclick = () => controller.toggleTrack(editableLane.id);
    } else {
      if (trackToggle === focused) canvas.focus({ preventScroll: true });
      trackToggle.hidden = true;
    }
    const visible = new Set(view.rows.map((row) => row.id));
    for (const [id, button] of rows) {
      if (visible.has(id)) continue;
      if (button === focused) canvas.focus({ preventScroll: true });
      rows.delete(id);
      button.remove();
    }
    let previous: HTMLButtonElement | undefined;
    for (const row of view.rows) {
      let button = rows.get(row.id);
      if (!button) {
        button = element('button', 'row', 'timeline-row');
        button.type = 'button';
        button.onclick = () => controller.selectRow(row.id);
        rows.set(row.id, button);
      }
      // Keep focused buttons in place. Reinsert only when the order changes.
      const next = previous ? previous.nextElementSibling : labels.firstElementChild;
      if (next !== button) labels.insertBefore(button, next);
      previous = button;
      button.style.top = (row.top - bodyTop) + 'px';
      button.style.height = row.height + 'px';
      button.style.paddingLeft = (10 + 12 * row.depth) + 'px';
      button.dataset.kind = row.kind;
      button.dataset.trackEditable = String(!!row.track?.editable);
      button.dataset.trackDisabled = String(row.track?.enabled === false);
      if (row.expandable) button.setAttribute('aria-expanded', String(row.expanded));
      else button.removeAttribute('aria-expanded');
      const label = rowLabel(row);
      if (button.textContent !== label) button.textContent = label;
      const edits = row.clips.filter(clip => clip.changedRanges.length).map(clip =>
        clip.placement.name + ' ' + clip.changedRanges.map(([start, end]) =>
          formatElapsedTime(start, controller.fps) + '–' + formatElapsedTime(end, controller.fps)).join(', '));
      button.title = label;
      if (edits.length) button.setAttribute('aria-description', 'Changed in last save: ' + edits.join(', '));
      else button.removeAttribute('aria-description');
    }
    // Moving a DOM node can drop focus even when the row is still visible.
    if (focused instanceof HTMLButtonElement && document.activeElement !== focused &&
        labels.contains(focused)) focused.focus({ preventScroll: true });
  }

  function selection() {
    for (const row of controller.layout().rows) {
      rows.get(row.id)?.setAttribute('aria-pressed', String(row.selected || controller.selection?.id === row.id));
    }
  }

  function cursor(view: TimelineLayout) {
    const frame = controller.cursor;
    const elapsed = formatElapsedTime(frame, controller.fps);
    time.value = elapsed;
    const [first, last] = view.visible;
    playhead.hidden = frame < first || frame >= last;
    playhead.style.transform = `translateX(${controller.frameToX(frame) - view.gutter}px)`;
    canvas.setAttribute('aria-valuemin', '0');
    canvas.setAttribute('aria-valuemax', String(controller.duration - 1));
    canvas.setAttribute('aria-valuenow', String(frame));
    canvas.setAttribute('aria-valuetext', elapsed);
    const hover = controller.skimFrame;
    skimmer.hidden = hover === null || hover < first || hover >= last;
    if (hover !== null) {
      skimmer.style.transform = `translateX(${controller.frameToX(hover) - view.gutter}px)`;
    }
  }

  return { root, plot, canvas, handle, layout, selection, cursor, flashChanges };
}
