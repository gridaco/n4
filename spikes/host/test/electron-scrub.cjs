// Electron test entry. Uses the ordinary harness and its real pointer bindings.
// Replay one SDL --scrub-trial report; input scheduling never waits for a picture.
const { app } = require('electron');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { setTimeout: sleep } = require('node:timers/promises');

const control = JSON.parse(fs.readFileSync(process.env.N4_SDL_REPORT, 'utf8'));
const trace = control.scrub_trace;
const targets = trace.inputs_us_frame.map(([, frame]) => frame);
const endFrame = targets.at(-1);
const deadline = setTimeout(() => {
  console.error('Electron scrub timed out');
  app.exit(1);
}, 120_000);

async function exercise(win) {
  // A hidden Electron window suppresses rAF and intentionally pauses playback.
  // Keep the measurement surface visible across the separate SDL/Electron launches.
  win.setAlwaysOnTop(true, 'floating');
  app.focus({ steal: true });
  win.show();
  win.focus();
  const wc = win.webContents;
  const js = (code) => wc.executeJavaScript(code);
  async function waitFor(expression, message) {
    for (let i = 0; i < 400; i++) {
      if (await js(expression)) return;
      await sleep(50);
    }
    throw Error(message + ': ' + await js("JSON.stringify({status: document.querySelector('[role=status]')?.textContent, visibility: document.visibilityState, timeline: document.querySelector('canvas[data-role=timeline]')?.dataset, movie: document.querySelector('canvas[data-role=preview]')?.dataset, play: document.querySelector('[data-role=play]')?.textContent})"));
  }
  // Wait for the initial parked picture and fitted ruler. No test-only seek API.
  let ready = false;
  for (let attempt = 0; attempt < 400; attempt++) {
    ready = await js("!!document.querySelector('canvas[data-role=preview][data-frame]') && !!document.querySelector('canvas[data-role=timeline]')");
    if (ready) break;
    await sleep(100);
  }
  assert.ok(ready, 'The harness did not present its first picture');
  await waitFor("document.querySelector('canvas[data-role=timeline]').dataset.thumbnailsReady === 'true'", 'Clip thumbnails did not load');
  const box = await js("document.querySelector('canvas[data-role=timeline]').getBoundingClientRect().toJSON()");
  const duration = control.project_frames;
  assert.ok(box.width >= duration, 'Test window must expose at least one CSS pixel per project frame');
  const point = frame => ({
    x: Math.round(box.x + (frame + .5) * box.width / duration),
    y: Math.round(box.y + 19),
  });
  const mouse = (type, frame) => wc.sendInputEvent({ type, ...point(frame),
    button: 'left', clickCount: 1, modifiers: type === 'mouseMove' ? ['leftButtonDown'] : [] });
  mouse('mouseDown', targets[0]);
  mouse('mouseUp', targets[0]);
  for (let i = 0; i < 150; i++) {
    if (await js("Number(document.querySelector('canvas[data-role=preview]').dataset.frame)") === targets[0]) break;
    await sleep(20);
  }
  // All observations below use the renderer's monotonic clock. Canvas data-frame
  // is updated immediately after submission. Physical scan-out is unobserved.
  await js(`(() => {
    const preview = document.querySelector('canvas[data-role=preview]');
    const timeline = document.querySelector('[data-role=timeline-root]');
    if (+preview.dataset.frame !== ${targets[0]}) throw Error('Initial seek failed: '+preview.dataset.frame);
    const start = performance.now(), time = () => Math.round((performance.now()-start)*1000);
    const trace = window.trialTrace = {visibility: [[0, document.visibilityState]], inputs: [], pictures: [[0, ${targets[0]}, ${targets[0]}]]};
    document.addEventListener('visibilitychange', () => trace.visibility.push([time(), document.visibilityState]));
    let target = ${targets[0]};
    for (const kind of ['pointerdown','pointermove','pointerup']) {
      timeline.addEventListener(kind, event => {
        if (kind === 'pointermove' && !(event.buttons & 1)) return;
        target = Math.floor((event.clientX - ${box.x}) * ${duration} / ${box.width});
        trace.inputs.push([time(), target]);
      }, true);
    }
    new MutationObserver(() => trace.pictures.push([time(), target, +preview.dataset.frame]))
      .observe(preview, {attributes:true, attributeFilter:['data-frame']});
  })()`);
  const start = performance.now();
  for (let i = 0; i < targets.length; i++) {
    const delay = start + i * 1000 / trace.pointer_hz - performance.now();
    if (delay > 0) await sleep(delay);
    mouse(i === 0 ? 'mouseDown' : i === targets.length - 1 ? 'mouseUp' : 'mouseMove', targets[i]);
  }
  let result;
  for (let i = 0; i < 200; i++) {
    result = await js('window.trialTrace');
    if (result.inputs.at(-1)?.[1] === endFrame && result.pictures.at(-1)[2] === endFrame &&
        result.pictures.at(-1)[0] >= result.inputs.at(-1)[0]) break;
    await sleep(20);
  }
  // Chromium can coalesce moves or omit moves that keep the same coordinates.
  // Keep delivered samples and original schedule so that cost stays visible.
  if (result.pictures.at(-1)[2] !== endFrame) {
    const parked = await js("document.querySelector('canvas[data-role=timeline]').getAttribute('aria-valuenow')");
    throw new Error(`Scrub did not land on ${endFrame}. Parked at ${parked}, drew ${result.pictures.at(-1)[2]}.`);
  }
  assert.equal(result.inputs.at(-1)[1], endFrame);
  const pixelHash = await js(`(async () => {
    const canvas = document.querySelector('canvas[data-role=preview]');
    const rgba = canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data;
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', rgba));
    return Array.from(digest, b => b.toString(16).padStart(2, '0')).join('');
  })()`);
  const temporary = fs.mkdtempSync(path.join(tmpdir(), 'n4-display-'));
  try {
    const dir = path.join(temporary, 'reference');
    const engine = path.join(__dirname, '../../engine/target/debug/n4');
    execFileSync(engine, ['render', process.argv[2], '--frames', String(endFrame), '--out', dir]);
    const png = fs.readdirSync(dir).find(f => f.endsWith('.png'));
    const rgba = execFileSync('ffmpeg', ['-v','error','-i',path.join(dir,png),'-pix_fmt','rgba','-f','rawvideo','pipe:1'], {maxBuffer: 40*1024*1024});
    assert.equal(pixelHash, createHash('sha256').update(rgba).digest('hex'));
  } finally {
    fs.rmSync(temporary, {recursive: true, force: true});
  }
  await js("document.querySelector('[data-role=play]').click()");
  const playbackTarget = Math.min(duration - 1, endFrame + 5);
  await waitFor(`Number(document.querySelector('canvas[data-role=preview]').dataset.frame) >= ${playbackTarget}`, 'Playback did not advance from the release frame');
  mouse('mouseDown', endFrame);
  mouse('mouseUp', endFrame);
  await waitFor(`document.querySelector('[data-role=play]').textContent.trim() === 'Play' && +document.querySelector('canvas[data-role=preview]').dataset.frame === ${endFrame}`, 'Seek during playback failed');
  const visibility = await js('window.trialTrace.visibility');
  assert.ok(visibility.every(([, state]) => state === 'visible'), 'Window was occluded during measurement');
  const report = {boundary: 'DOM pointer dispatch to canvas submission (MutationObserver)',
    duration, schedulingHz: trace.pointer_hz, scheduled: targets, ...result,
    devicePixelRatio: await js('devicePixelRatio'), window: box,
    electronVersion: process.versions.electron, pixelHash};
  fs.writeFileSync(process.env.N4_ELECTRON_REPORT, JSON.stringify(report) + '\n');
  console.log('Electron landed on', endFrame, 'with', result.inputs.length, 'delivered pointer events');
}

app.once('browser-window-created', (_, win) => {
  win.webContents.once('did-finish-load', () => exercise(win).then(() => {
    clearTimeout(deadline);
    win.close();
  }, error => {
    console.error(error);
    app.exit(1);
  }));
});
require('../harness/dist/main.cjs');
