// Run from spikes/host with pnpm exec electron test/electron-export.cjs.
// Only destination selection is scripted. The harness, native host and encode are real.
const { app, dialog } = require('electron');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { once } = require('node:events');
const { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { setTimeout: sleep } = require('node:timers/promises');

const temp = mkdtempSync(join(tmpdir(), 'n4-electron-export-'));
const width = 1280;
const height = 720;
const frames = 120;
const project = join(temp, 'project');
const destination = join(temp, 'exports');
const movie = join(destination, 'N4 video.mp4');
const cancelledMovie = join(destination, 'Cancelled.mp4');
const closedMovie = join(destination, 'Closed.mp4');
const quitMovie = join(destination, 'Quit.mp4');
const detail = join(project, 'sequences', 'picture.xml');
const plate = join(project, 'plate.png');
mkdirSync(join(project, 'sequences'), { recursive: true });
mkdirSync(destination);
app.setPath('userData', join(temp, 'profile'));
app.once('quit', () => rmSync(temp, { recursive: true, force: true }));

function ffmpeg(...args) {
  return execFileSync('ffmpeg', ['-nostdin', '-v', 'error', ...args], {
    timeout: 20_000, maxBuffer: 8 * 1024 * 1024,
  });
}

function setArtwork(background, color) {
  writeFileSync(detail, `<sequence name="Picture" background="${background}">
    <image name="Plate" source="plate.png"/>
  </sequence>`);
  ffmpeg('-y', '-f', 'lavfi', '-i', `color=c=${color}:s=${width / 2}x${height}`,
    '-frames:v', '1', '-threads', '1', '-update', '1', plate);
}

writeFileSync(join(project, 'project.proj'),
  `project Export-test fps 24000/1001 size ${width}x${height}\n[0,${frames}) V1 picture\n[0,${frames}) A1 tone\n`);
writeFileSync(join(project, 'sequences', 'tone.json'), JSON.stringify({ source: 'tone.wav', source_in_sample: 0 }));
function setTone(frequency) {
  ffmpeg('-y', '-f', 'lavfi', '-i', `sine=frequency=${frequency}:sample_rate=48000:duration=6`, join(project, 'tone.wav'));
}
setTone(440);
setArtwork('#00ff00', 'red');

const choices = [null, movie.slice(0, -4), cancelledMovie, movie, closedMovie, quitMovie];
let visits = 0;
let currentExportWindow;
dialog.showSaveDialog = async (window, options) => {
  assert.equal(window, currentExportWindow, 'The native picker belongs to the Export Video window');
  assert.match(options.title, /^export video$/i);
  assert.match(options.defaultPath, /\.mp4$/i);
  assert.deepEqual(options.filters.map(filter => filter.extensions), [['mp4']]);
  assert.ok(visits < choices.length, 'Unexpected save dialog');
  const filePath = choices[visits++];
  return filePath ? { canceled: false, filePath } : { canceled: true };
};

function checkMovie() {
  const probe = JSON.parse(execFileSync('ffprobe', [
    '-v', 'error', '-show_streams', '-show_format', '-of', 'json', movie,
  ]));
  assert.equal(probe.streams.length, 2);
  const video = probe.streams.find(stream => stream.codec_type === 'video');
  const audio = probe.streams.find(stream => stream.codec_type === 'audio');
  assert.equal(video.codec_name, 'h264');
  assert.deepEqual([video.width, video.height], [width, height]);
  assert.equal(video.avg_frame_rate, '24000/1001');
  assert.equal(Number(video.nb_frames), frames);
  assert.equal(video.pix_fmt, 'yuv420p');
  assert.equal(video.color_space, 'bt709');
  assert.equal(audio.codec_name, 'aac');
  assert.equal(Number(audio.sample_rate), 48_000);
  assert.equal(audio.channels, 2);
  assert.ok(Math.abs(Number(probe.format.duration) - frames * 1001 / 24000) < 0.05);

  // The file must contain the accepted red raster and green background even
  // though the inputs changed while the accepted revision was exporting.
  const pixels = ffmpeg('-i', movie, '-frames:v', '1', '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1');
  function sample(x, y) {
    const i = (y * width + x) * 3;
    return [...pixels.subarray(i, i + 3)];
  }
  const [red, rGreen, rBlue] = sample(width / 4, height / 2);
  assert.ok(red > 180 && rGreen < 90 && rBlue < 90, 'Expected the accepted red foreground');
  const [gRed, green, gBlue] = sample(width * 3 / 4, height / 2);
  assert.ok(green > 180 && gRed < 90 && gBlue < 90, 'Expected the accepted green background');

  const pcm = ffmpeg('-i', movie, '-map', '0:a:0', '-ac', '1', '-t', '0.1', '-f', 's16le', 'pipe:1');
  const samples = new Int16Array(pcm.buffer, pcm.byteOffset, pcm.length / 2);
  const rms = Math.sqrt(samples.reduce((sum, sample) => sum + sample * sample, 0) / samples.length);
  const periods = samples.reduce((count, sample, i) =>
    count + Number(sample > 0 && samples[i - 1] <= 0), 0);
  assert.ok(rms > 500 && Math.abs(periods - 44) <= 2, 'Expected the authored tone in export');
}

async function exercise(win) {
  win.setAlwaysOnTop(true, 'floating');
  app.focus({ steal: true });
  win.show();
  win.focus();
  const wc = win.webContents;
  const viewer = (script) => wc.executeJavaScript(script);
  const exportView = (script) => currentExportWindow.webContents.executeJavaScript(script);
  const exportButton = "document.querySelector('[data-role=export]')";
  const startButton = "document.querySelector('[data-role=start-export]')";
  const cancelButton = "document.querySelector('[data-role=cancel-export]')";
  const progress = "document.querySelector('[data-role=export-progress]')";
  const preview = "document.querySelector('[data-role=preview]')";
  const status = "document.querySelector('[role=status]').textContent";
  const running = `${progress}.getClientRects().length > 0 &&
    ${progress}.max === ${frames} && ${startButton}.disabled`;
  async function waitFor(label, check) {
    const stop = performance.now() + 30_000;
    while (performance.now() < stop) {
      if (await check()) return;
      await sleep(20);
    }
    const message = currentExportWindow && !currentExportWindow.isDestroyed()
      ? await exportView(status) : await viewer(status);
    throw new Error(`Timed out waiting for ${label}. ${message}`);
  }

  await waitFor('first picture and Export control', () =>
    viewer(`+(${preview}.dataset.frame ?? -1) === 0 && !${exportButton}.disabled`));
  assert.equal(await viewer(`Boolean(${progress} || ${cancelButton})`), false);
  assert.equal(await viewer('typeof window.n4Viewer.startExport'), 'undefined');
  const originalRevision = await viewer(`+${preview}.dataset.revision`);

  async function openExport() {
    const opened = once(app, 'browser-window-created');
    await viewer(`${exportButton}.click()`);
    const [, child] = await opened;
    currentExportWindow = child;
    assert.equal(child.getParentWindow(), win);
    assert.ok(child.isModal(), 'Export opens as a modal child of the viewer');
    await waitFor('Export Video controls', () =>
      child.isVisible() && exportView(`!!${startButton} && !${startButton}.disabled`));
    assert.match(child.getTitle(), /^export video$/i);
    assert.equal(await exportView("document.querySelector('[data-field=size]').textContent"), '1280 × 720');
    assert.equal(await exportView("document.querySelector('[data-field=duration]').textContent"), '0:05.005');
    return child;
  }

  const child = await openExport();
  assert.equal(visits, 0, 'Opening Export Video waits before showing the destination picker');

  await exportView(`${startButton}.click()`);
  await waitFor('dismissed save dialog', async () => visits === 1 && await exportView(`!${startButton}.disabled`));
  assert.deepEqual(readdirSync(destination), []);

  await exportView(`${startButton}.click()`);
  await waitFor('visible export progress', () => exportView(running));
  setArtwork('#0000ff', 'white');
  setTone(880);
  await waitFor('completed export', async () =>
    existsSync(movie) && await exportView(`!${startButton}.disabled`));
  assert.equal(await exportView(status), 'Exported N4 video.mp4');
  checkMovie();
  const encoded = readFileSync(movie);

  // The sheet stays open across completion and the deferred input refresh.
  await waitFor('deferred reload after export', () =>
    viewer(`+${preview}.dataset.revision > ${originalRevision} && !${exportButton}.disabled`));
  await exportView(`${startButton}.click()`);
  await waitFor('active export', () => exportView(`!${cancelButton}.disabled && !${cancelButton}.hidden`));
  await exportView(`${cancelButton}.click()`);
  await waitFor('cancelled export', () => exportView(`!${startButton}.disabled`));
  assert.equal(await exportView(status), 'Export cancelled');
  assert.ok(!existsSync(cancelledMovie), 'Cancellation published an output file');

  await exportView(`${startButton}.click()`);
  await waitFor('reported output collision', async () =>
    visits === 4 && await exportView(`!${startButton}.disabled && /exist|new (?:file|name)|replace/i.test(${status})`));
  assert.deepEqual(readFileSync(movie), encoded);
  assert.deepEqual(readdirSync(destination), ['N4 video.mp4']);

  await exportView(`${startButton}.click()`);
  await waitFor('export in progress before closing its window', () => exportView(running));
  const closed = once(child, 'closed');
  child.close();
  await closed;
  assert.deepEqual(readdirSync(destination), ['N4 video.mp4']);

  // The same viewer and sidecar can seek once the export window closes.
  win.focus();
  const box = await viewer("document.querySelector('canvas[data-role=timeline]').getBoundingClientRect().toJSON()");
  const x = Math.round(box.x + 20.5 * box.width / frames);
  const y = Math.round(box.y + 19);
  for (const type of ['mouseDown', 'mouseUp']) wc.sendInputEvent({ type, button: 'left', clickCount: 1, x, y });
  await waitFor('preview seek after export', () => viewer(`+${preview}.dataset.frame === 20`));

  const pending = await openExport();
  await exportView(`${startButton}.click()`);
  await waitFor('export in progress before closing its parent', () => exportView(running));
  return pending;
}

const deadline = setTimeout(() => {
  console.error('Electron export test timed out');
  app.exit(1);
}, 150_000);
app.once('browser-window-created', (_, win) => {
  win.webContents.once('did-finish-load', () => exercise(win).then((child) => {
    win.once('closed', () => {
      clearTimeout(deadline);
      try {
        assert.ok(child.isDestroyed(), 'Closing the viewer also closes its Export Video window');
        assert.equal(visits, choices.length);
        assert.deepEqual(readdirSync(destination), ['N4 video.mp4']);
        console.log('Export Video verified H.264/AAC, accepted assets, cancellation, collision safety, and timeline seek after closing the modal.');
      } catch (error) {
        console.error(error);
        app.exit(1);
      }
    });
    win.close();
  }, error => {
    console.error(error);
    app.exit(1);
  }));
});
process.argv[2] = project;
require('../harness/dist/main.cjs');
