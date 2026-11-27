// Real filesystem, headless CLI, native loader and rendered pixels. Run with just host-test.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, mkdtemp, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as wait } from 'node:timers/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

test('external saves update a parked picture, recover a missing linked asset and clamp edits to duration', { timeout: 45_000 }, async t => {
  const dir = await mkdtemp(join(tmpdir(), 'n4-live-'));
  const project = join(dir, 'film');
  const master = join(project, 'project.proj');
  const leaf = join(project, 'sequences', 'map.xml');
  const artwork = join(dir, 'shared.svg');
  const timeline = end => 'project test fps 24/1 size 64x32\n[0,' + end + ') V1 map\n';
  await mkdir(join(project, 'sequences'), { recursive: true });
  await writeFile(master, timeline(12));
  await writeFile(leaf, '<sequence background="#ffffff"/>');
  const child = spawn(process.execPath, [fileURLToPath(new URL('../watch-project.mjs', import.meta.url)), project, '9'],
    { stdio: ['ignore', 'pipe', 'pipe'] });
  const closed = once(child, 'close');
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  let stderr = '';
  child.stderr.setEncoding('utf8').on('data', data => { stderr += data; });
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await closed;
    await rm(dir, { recursive: true, force: true });
  });
  async function next(event, revision, frame) {
    const line = await lines.next();
    assert.ok(!line.done, 'watch command stopped: ' + stderr);
    const result = JSON.parse(line.value);
    assert.deepEqual([result.event, result.revision, result.frame], [event, revision, frame], line.value);
    if (event === 'refreshed') {
      const map = [{path:'V1:map@0', ranges:[[0,12]]}];
      assert.deepEqual(result.change, {
        fromRevision: revision - 1, toRevision: revision,
        damage: {picture:{kind:'all'}, audio:{kind:revision === 3 ? 'all' : 'none'}},
        timeline: revision === 3 ? {kind:'unknown'} : {kind:'known', occurrences:map, thumbnails:map},
      });
    }
    return result;
  }
  const ready = await next('ready', 0, 9);
  await writeFile(join(project, 'review-notes.txt'), 'unrelated');
  await wait(800); // Any spurious revision will be the next line read below.
  await writeFile(leaf, '<sequence');
  assert.match((await next('rejected', 0, 9)).error, /map\.xml/);

  await writeFile(leaf, '<sequence background="#ffffff"><svg name="Map" position="0 0" source="../shared.svg"/></sequence>');
  assert.match((await next('rejected', 0, 9)).error, /shared\.svg/);
  const art = color => '<svg width="64" height="32">' +
    '<path d="M0 0H64V32H0Z" fill="' + color + '"/></svg>';
  await writeFile(artwork, art('#ff0000'));
  const red = await next('refreshed', 1, 9);
  assert.notEqual(red.rgbaSha256, ready.rgbaSha256);
  await writeFile(artwork + '.tmp', art('#00ff00'));
  await rename(artwork + '.tmp', artwork);
  const green = await next('refreshed', 2, 9);
  assert.notEqual(green.rgbaSha256, red.rgbaSha256);
  await writeFile(master, timeline(4));
  assert.equal((await next('refreshed', 3, 3)).rgbaSha256, green.rgbaSha256);
});
