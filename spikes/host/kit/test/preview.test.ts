import assert from 'node:assert/strict';
import test from 'node:test';
import { setImmediate } from 'node:timers/promises';
import { PreviewController } from '../src/view/preview.ts';
import type { Picture, ViewerDocument, ViewerBridge, PlaybackSample } from '../src/client/types.ts';

function picture(frame: number, revision: number, generation = 1): Picture {
  return { frame, revision, generation, width: 1, height: 1, rgba: new Uint8Array(4) };
}
function document(revision: number): ViewerDocument {
  return { sessionId: 'test', revision, size: [1, 1], peaks: [], snapshot: {
    name: 'test', fps: [30, 1], duration: 100, placements: [], definitions: {},
  } };
}
function rig() {
  const seeks: { frame: number; deliver: (p: Picture) => void }[] = [];
  const starts: number[] = [];
  const seeking: boolean[] = [];
  const drawn: number[] = [], ack: number[] = [], errors: unknown[] = [];
  let sendSample: ((sample: PlaybackSample) => void) | undefined;
  const bridge: ViewerBridge = {
    async inspect() { return document(1); },
    async reopen() { return {
      ...document(2), parked: picture(0, 2), change: {fromRevision: 1, toRevision: 2, timeline: {kind: 'unknown'}, damage: {picture: {kind: 'all'}, audio: {kind: 'all'}}},
    }; },
    seek(frame) { return new Promise(deliver => seeks.push({frame, deliver})); },
    async thumbnail() { throw Error('unexpected thumbnail'); },
    async play(frame) { starts.push(frame); }, async pause() {},
    next() { return new Promise(done => { sendSample = done; }); },
    async presented(frame) { ack.push(frame); },
  };
  const player = new PreviewController(bridge, document(1),
    { picture() {}, playback() {}, seeking(active) { seeking.push(active); }, error(e) { errors.push(e); } },
    p => drawn.push(p.frame), () => setImmediate());
  return {player, seeks, starts, seeking, drawn, ack, errors, send: (p: Picture) => sendSample!({picture:p,playing:true})};
}
test('scrub queues one latest target and discards a late result across exact landing', async () => {
  const r = rig();
  r.player.seek(1,'scrub'); r.player.seek(5,'scrub'); r.player.seek(7,'exact');
  assert.deepEqual(r.seeks.map(s=>s.frame), [1]);
  r.seeks[0].deliver(picture(1,1)); await setImmediate();
  assert.deepEqual(r.seeks.map(s=>s.frame), [1,7]);
  r.seeks[1].deliver(picture(7,1)); await setImmediate();
  assert.deepEqual(r.drawn,[7]);
});
test('replacing a document discards in-flight seeks',async () => {
  const r = rig();
  r.player.seek(2,'exact'); r.player.replace(document(2), picture(2,2)); r.player.seek(3,'exact');
  r.seeks[0].deliver(picture(2,1)); await setImmediate();
  r.seeks[1].deliver(picture(3,2)); await setImmediate();
  assert.deepEqual(r.drawn,[2,3]);
});
test('a paused generation cannot present a late playback picture or resume audio',async () => {
  const r = rig();
  const playing = r.player.play(0); await setImmediate();
  r.send(picture(0,1)); await setImmediate();
  assert.deepEqual(r.drawn,[0]); assert.deepEqual(r.ack,[0]);
  r.player.pause();
  r.send(picture(1,1)); await playing;
  assert.deepEqual(r.drawn,[0]); assert.deepEqual(r.ack,[0]); assert.deepEqual(r.errors,[]);
});

test('skim coalesces requests and leaving instantly restores the cached picture', async () => {
  const r = rig();
  r.player.seek(12, 'exact');
  r.seeks[0].deliver(picture(12, 1));
  await setImmediate();
  r.player.skim(23);
  r.player.skim(24);
  r.player.skim(25);
  assert.deepEqual(r.seeks.map(s => s.frame), [12, 23]);
  r.seeks[1].deliver(picture(23, 1));
  await setImmediate();
  assert.deepEqual(r.drawn, [12, 23]);
  assert.deepEqual(r.seeks.map(s => s.frame), [12, 23, 25]);
  assert.deepEqual(r.seeking, [true, false, true]);
  r.player.clearSkim();
  assert.deepEqual(r.drawn, [12, 23, 12]);
  r.seeks[2].deliver(picture(25, 1));
  await setImmediate();
  assert.deepEqual(r.drawn, [12, 23, 12]);
  assert.equal(r.seeks.length, 3);
});

test('leaving restores the exact parked target if its picture was still pending', async () => {
  const r = rig();
  r.player.seek(12, 'exact');
  r.player.skim(23);
  r.seeks[0].deliver(picture(12, 1));
  await setImmediate();
  r.player.pause();
  r.seeks[1].deliver(picture(23, 1));
  await setImmediate();
  assert.deepEqual(r.seeks.map(s => s.frame), [12, 23, 12]);
  r.seeks[2].deliver(picture(12, 1));
  await setImmediate();
  assert.deepEqual(r.drawn, [12]);
});

test('a committed scrub and a reopened revision both supersede a pending skim', async () => {
  const r = rig();
  r.player.skim(23);
  r.player.seek(12, 'scrub');
  r.seeks[0].deliver(picture(23, 1));
  await setImmediate();
  r.seeks[1].deliver(picture(12, 1));
  await setImmediate();
  r.player.skim(24);
  r.player.replace(document(2), picture(15, 2));
  r.seeks[2].deliver(picture(24, 1));
  await setImmediate();
  assert.deepEqual(r.drawn, [12, 15]);
  r.player.skim(25);
  r.player.dispose();
  r.seeks[3].deliver(picture(25, 2));
  await setImmediate();
  assert.deepEqual(r.drawn, [12, 15]);
});

test('playback starts at the skim frame and ignores hover while running', async () => {
  const r = rig();
  r.player.seek(12, 'exact');
  r.seeks[0].deliver(picture(12, 1));
  await setImmediate();
  r.player.skim(23);
  r.seeks[1].deliver(picture(23, 1));
  await setImmediate();
  // The thumbnail queue can resume while frame 23 is still held for playback.
  assert.deepEqual(r.seeking, [true, false, true, false]);
  const playing = r.player.play(12);
  r.player.skim(25);
  assert.deepEqual(r.starts, [23]);
  assert.deepEqual(r.drawn, [12, 23]);
  assert.equal(r.seeks.length, 2);
  await setImmediate();
  r.send(picture(24, 1));
  await setImmediate();
  r.player.pause();
  r.send(picture(25, 1));
  await playing;
  assert.deepEqual(r.drawn, [12, 23, 24]);
  r.player.skim(25);
  r.seeks[2].deliver(picture(25, 1));
  await setImmediate();
  r.player.clearSkim();
  assert.deepEqual(r.drawn, [12, 23, 24, 25, 24]);
  const resumed = r.player.play(24);
  assert.deepEqual(r.starts, [23, 24]);
  await setImmediate();
  r.player.pause();
  r.send(picture(25, 1));
  await resumed;
});

test('a quick pause lands on the latest skim target even with a seek still pending', async () => {
  const r = rig();
  r.player.replace(document(2), picture(12, 2));
  r.player.skim(23);
  r.player.skim(25);
  const playing = r.player.play(12);
  assert.deepEqual(r.starts, [25]);
  await setImmediate();
  r.player.pause();
  r.seeks[0].deliver(picture(23, 2));
  r.send(picture(25, 2));
  await playing;
  assert.deepEqual(r.drawn, [12]);
  assert.deepEqual(r.seeks.map(({ frame }) => frame), [23, 25]);
  r.seeks[1].deliver(picture(25, 2));
  await setImmediate();
  assert.deepEqual(r.drawn, [12, 25]);
});
