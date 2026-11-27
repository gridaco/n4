import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { NativeSession } from '@n4/host-kit/client';

const spikes = fileURLToPath(new URL('../../', import.meta.url));
const bin = process.env.N4_HOST_BIN || join(spikes, 'engine/target/debug/n4-host');

test('the editable sidecar shares parked picture, native HUD, and saved track state', { timeout: 30_000 }, async t => {
  const project = await mkdtemp(join(tmpdir(), 'n4-native-editor-'));
  const master = join(project, 'project.proj');
  const sequences = join(project, 'sequences');
  const original = 'project editor fps 24/1 size 64x36\n[0,12) V1 plate\n[0,7) V2 badge\n';
  await mkdir(sequences);
  await writeFile(master, original);
  await writeFile(join(sequences, 'plate.xml'),
    '<sequence background="#000000"><rect x="37" y="15" width="12" height="10" fill="#ff6600"/></sequence>');
  await writeFile(join(sequences, 'badge.xml'),
    '<sequence><rect x="12" y="8" width="22" height="12" fill="#00ff00"/></sequence>');
  t.after(() => rm(project, { recursive: true, force: true }));
  const session = await NativeSession.open(bin, project, undefined, {editor:true});
  async function editor(command) {
    const response = await session.editor(command);
    assert.ok(!response.error, JSON.stringify(response));
    return response;
  }
  function pixel(picture) {
    const offset = (10 * picture.width + 16) * 4;
    return [...picture.rgba.subarray(offset, offset + 4)];
  }
  try {
    assert.equal(session.document.editable, true);
    const initial = await editor({op:'getState'});
    assert.equal(initial.result.sessionId, session.document.sessionId);
    assert.equal((await session.hud(0, 3)).tile, null);
    assert.deepEqual(pixel(await session.seek(3, 1)), [0, 255, 0, 255]);
    assert.deepEqual(pixel(await session.seek(8, 2, true)), [0, 0, 0, 255]);

    const hover = await editor({op:'hoverAt', revision:0, frame:3, point:[16,10]});
    assert.deepEqual(hover.result.hover, {occurrence:'V2:badge@0', layers:[0]});
    assert.deepEqual(hover.result.selections, []);
    assert.equal(hover.result.canUndo, false);
    assert.ok((await session.hud(0, 3)).tile);
    assert.equal((await session.hud(0, 8)).tile, null);
    await editor({op:'clearHover', revision:0});
    assert.equal((await session.hud(0, 3)).tile, null);

    await editor({op:'beginMarquee', revision:0, frame:3, point:[52,27]});
    const box = await editor({op:'updateMarquee', revision:0, point:[10,6]});
    assert.deepEqual(box.result.marquee, [10,6,52,27]);
    assert.deepEqual(box.result.selections, []);
    assert.ok((await session.hud(0, 3)).tile);
    const multiple = await editor({op:'finishMarquee', revision:0, point:[10,6]});
    assert.deepEqual(multiple.result.selections.map(target => target.occurrence), ['V2:badge@0', 'V1:plate@0']);
    assert.equal(multiple.result.marquee, null);
    assert.equal(multiple.result.dirty, false);
    assert.equal(multiple.result.canUndo, false);

    const selection = await editor({op:'selectAt', revision:0, frame:3, point:[16,10]});
    assert.equal(selection.result.frame, 3);
    assert.deepEqual(selection.result.selection, {kind:'layer', occurrence:'V2:badge@0', layers:[0]});
    const hud = await session.hud(0, 3);
    assert.equal(hud.hudVersion, selection.result.hudVersion);
    assert.ok(hud.tile);
    assert.ok(hud.tile.x > 0 && hud.tile.y > 0);
    assert.ok(hud.tile.width < 64 && hud.tile.height < 36);
    assert.ok(hud.tile.rgba.some((v, i) => i % 4 === 3 && v !== 0));
    assert.ok(hud.tile.rgba.some((v, i) => i % 4 === 3 && v === 0));
    assert.equal((await session.hud(0, 8)).tile, null);

    const disabled = await editor({op:'setTrackEnabled', revision:0, scope:'root', track:'V2', enabled:false});
    assert.equal(disabled.transition.parked.frame, 3);
    assert.deepEqual(pixel(disabled.transition.parked), [0, 0, 0, 255]);
    assert.equal(disabled.result.state.dirty, true);
    assert.equal((await session.hud(1, 3)).tile, null);
    assert.equal(await readFile(master, 'utf8'), original);
    const undo = await editor({op:'undo', revision:1});
    assert.deepEqual(pixel(undo.transition.parked), [0, 255, 0, 255]);
    assert.ok((await session.hud(2, 3)).tile);
    const redo = await editor({op:'redo', revision:2});
    assert.deepEqual(pixel(redo.transition.parked), [0, 0, 0, 255]);
    assert.equal((await editor({op:'save', revision:3})).result.dirty, false);
    assert.equal(await readFile(master, 'utf8'), original.replace('\n', '\ntrack V2 off\n'));
  } finally {
    await session.close();
  }
  const reopened = await NativeSession.open(bin, project, undefined, {editor:true});
  try {
    const state = await reopened.editor({op:'getState'});
    assert.equal(state.result.tracks.find(track => track.track === 'V2').enabled, false);
    assert.deepEqual(pixel(await reopened.seek(3, 1)), [0, 0, 0, 255]);
  } finally {
    await reopened.close();
  }
});
