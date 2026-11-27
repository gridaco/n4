/** Resizes the timeline pane while CSS keeps both panes within the window. */
export function mountTimelineDivider(
  layout: HTMLElement,
  divider: HTMLElement,
  preview: HTMLElement,
  timeline: HTMLElement,
) {
  const style = getComputedStyle(layout);
  const min = parseInt(style.getPropertyValue('--timeline-min-height'), 10);
  const minPreview = parseInt(style.getPropertyValue('--preview-min-height'), 10);
  const max = () => Math.max(min, timeline.clientHeight + preview.clientHeight - minPreview);
  let drag: { id: number; y: number; height: number } | undefined;

  function updateAccessibleSize() {
    divider.setAttribute('aria-valuemax', String(max()));
    divider.setAttribute('aria-valuenow', String(timeline.clientHeight));
  }

  function resize(height: number) {
    layout.style.setProperty('--timeline-height', Math.min(max(), Math.max(min, height)) + 'px');
    updateAccessibleSize();
  }

  const lifetime = new AbortController();
  const { signal } = lifetime;
  divider.setAttribute('aria-valuemin', String(min));
  divider.addEventListener('pointerdown', event => {
    if (event.button !== 0 || drag) return;
    divider.focus({ preventScroll: true });
    divider.setPointerCapture(event.pointerId);
    drag = { id: event.pointerId, y: event.clientY, height: timeline.clientHeight };
    divider.dataset.dragging = '';
    event.preventDefault();
  }, { signal });
  function move(event: PointerEvent) {
    if (drag?.id === event.pointerId) resize(drag.height + drag.y - event.clientY);
  }
  function release(event: PointerEvent) {
    if (drag?.id !== event.pointerId) return;
    drag = undefined;
    delete divider.dataset.dragging;
  }
  divider.addEventListener('pointermove', move, { signal });
  divider.addEventListener('pointerup', event => {
    move(event);
    release(event);
  }, { signal });
  divider.addEventListener('pointercancel', release, { signal });
  divider.addEventListener('lostpointercapture', release, { signal });
  divider.addEventListener('keydown', event => {
    switch (event.key) {
      case 'ArrowUp': resize(timeline.clientHeight + 16); break;
      case 'ArrowDown': resize(timeline.clientHeight - 16); break;
      case 'Home': resize(min); break;
      case 'End': resize(max()); break;
      default: return;
    }
    event.preventDefault();
  }, { signal });

  const observer = new ResizeObserver(updateAccessibleSize);
  observer.observe(layout);
  updateAccessibleSize();
  return () => {
    lifetime.abort();
    observer.disconnect();
  };
}
