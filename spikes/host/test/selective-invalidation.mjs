// Exercise actual seeks, disk edits and reloads of a shared authored sequence.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { cp, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { NativeSession } from '@n4/host-kit/client';

const spikes = fileURLToPath(new URL('../../', import.meta.url));
const bin = process.env.N4_HOST_BIN || join(spikes, 'engine/target/debug/n4-host');
const digest = picture => createHash('sha256').update(picture.rgba).digest('hex');
const none = {kind: 'none'};
const ranges = (...ranges) => ({kind: 'ranges', ranges});
const occurrence = (path, ...ranges) => ({path, ranges});
const firstPrint = 'V1:survey@0';
const secondPrint = 'V1:survey@128';

test('a shared grade, a soundtrack gain and a camera cut invalidate only their outputs', {timeout: 180_000}, async t => {
  const project = await mkdtemp(join(tmpdir(), 'n4-damage-'));
  t.after(() => rm(project, {recursive: true, force: true}));
  const master = join(project, 'project.proj');
  const sequences = join(project, 'sequences');
  await cp(join(spikes, 'examples/signal-atlas'), project, {recursive: true});
  const session = await NativeSession.open(bin, project);
  try {
    const peaks = session.document.peaks;
    assert.ok(peaks.some(value => value > 0));
    const before = new Map();
    for (const frame of [42, 140, 64]) before.set(frame, digest(await session.seek(frame)));
    const samples = [[firstPrint, 42], ['V2:transmission@0', 42], ['V1:analysis@64', 64],
      [secondPrint, 140], ['V2:transmission@128', 140]];
    async function isolatedSamples(s) {
      const hashes = [];
      for (const [path, frame] of samples) hashes.push(digest(await s.thumbnail(frame, 1, path)));
      return hashes;
    }
    const initialThumbnails = await isolatedSamples(session);
    const initial = await session.stats();
    assert.equal(initial.audioPreparations, 1);
    const print = join(sequences, 'survey.xml');
    const leaf = await readFile(print, 'utf8');
    const grade = leaf.replace('source="assets/relief.png"/>', 'source="assets/relief.png"><rgb-grade gain="1 1 0.65"/></image>');
    assert.notEqual(leaf, grade);
    await writeFile(print, grade);
    const graded = await session.reopen(42);
    assert.deepEqual(graded.change.damage, {picture: ranges([0, 64], [128, 192]), audio: none});
    const affected = [occurrence(firstPrint, [0, 64]), occurrence(secondPrint, [128, 192])];
    assert.equal(graded.change.timeline.kind, 'known');
    const byPath = entries => [...entries].sort((a,b) => a.path.localeCompare(b.path));
    assert.deepEqual(byPath(graded.change.timeline.occurrences), byPath(affected));
    assert.deepEqual(byPath(graded.change.timeline.thumbnails), byPath(affected));
    assert.deepEqual(graded.peaks, peaks);
    assert.notEqual(digest(graded.parked), before.get(42));
    const afterGrade = await session.stats();
    assert.equal(afterGrade.audioPreparations, initial.audioPreparations);
    assert.ok(afterGrade.frames.inherited > 0);
    const gap = digest(await session.seek(64));
    const afterGap = await session.stats();
    assert.equal(gap, before.get(64));
    assert.equal(afterGap.frames.hits, afterGrade.frames.hits + 1);
    assert.equal(afterGap.frames.misses, afterGrade.frames.misses);
    const reprise = digest(await session.seek(140));
    const afterReprise = await session.stats();
    assert.notEqual(reprise, before.get(140));
    assert.equal(afterReprise.frames.misses, afterGap.frames.misses + 1);
    const fresh = await NativeSession.open(bin, project);
    try {
      assert.notEqual(fresh.document.sessionId, session.document.sessionId);
      const updatedThumbnails = await isolatedSamples(session);
      assert.deepEqual(updatedThumbnails, await isolatedSamples(fresh));
      assert.deepEqual(samples.filter((_, i) => initialThumbnails[i] !== updatedThumbnails[i]).map(([path]) => path),
        [firstPrint, secondPrint]);
      for (const [frame, expected] of [[42, digest(graded.parked)], [64, gap], [140, reprise]]) {
        assert.equal(digest(await fresh.seek(frame)), expected, 'fresh frame ' + frame);
      }
    } finally { await fresh.close(); }
    t.diagnostic('Print grade: ' + JSON.stringify({damage: graded.change.damage,
      audioPreparations: afterGrade.audioPreparations - initial.audioPreparations,
      gapHits: afterGap.frames.hits - afterGrade.frames.hits,
      repriseMisses: afterReprise.frames.misses - afterGap.frames.misses,
      residentBytes: afterReprise.frames.bytes, budgetBytes: afterReprise.frames.budgetBytes}));

    const audio = join(sequences, 'reprise.json');
    const source = JSON.parse(await readFile(audio, 'utf8'));
    await writeFile(audio, JSON.stringify({...source, gain: 0}));
    const muted = await session.reopen(140);
    assert.deepEqual(muted.change.damage, {picture: none, audio: ranges([128, 192])});
    assert.deepEqual(muted.change.timeline, {kind:'known', occurrences:[occurrence('A1:reprise@128', [128, 192])], thumbnails:[]});
    assert.equal(digest(muted.parked), reprise);
    assert.notDeepEqual(muted.peaks, peaks);
    const afterGain = await session.stats();
    assert.equal(afterGain.audioPreparations, afterGrade.audioPreparations + 1);
    assert.equal(digest(await session.seek(64)), gap);
    assert.equal((await session.stats()).frames.hits, afterGain.frames.hits + 1);
    t.diagnostic('Reprise gain: ' + JSON.stringify({damage: muted.change.damage,
      audioPreparations: afterGain.audioPreparations - afterGrade.audioPreparations}));

    const timeline = await readFile(master, 'utf8');
    const beforeCut = new Map([[140, reprise]]);
    for (const frame of [139, 141]) beforeCut.set(frame, digest(await session.seek(frame)));
    const rolled = timeline.replace('[0,12) CAM wide', '[0,13) CAM wide')
      .replace('[12,48) CAM detail', '[13,48) CAM detail');
    assert.notEqual(timeline, rolled);
    await writeFile(master, rolled);
    const camera = await session.reopen(140);
    assert.deepEqual(camera.change.damage, {picture: ranges([140, 141]), audio: none});
    const cut = [occurrence(secondPrint, [140, 141])];
    assert.deepEqual(camera.change.timeline, {kind:'known', occurrences:cut, thumbnails:cut});
    assert.notEqual(digest(camera.parked), reprise);
    const oracle = await NativeSession.open(bin, project);
    try {
      assert.deepEqual(muted.peaks, oracle.document.peaks);
      for (const frame of [139, 140, 141]) {
        const pixels = digest(await session.seek(frame));
        assert.equal(pixels, digest(await oracle.seek(frame)));
        assert.equal(pixels !== beforeCut.get(frame), frame === 140);
      }
    } finally { await oracle.close(); }

    await writeFile(print, '<sequence');
    await assert.rejects(session.reopen(140), /survey\.xml/);
    assert.equal(session.document.revision, camera.revision);
    assert.equal(digest(await session.seek(140)), digest(camera.parked));
    await writeFile(print, grade + '\n');
    const formatted = await session.reopen(140);
    assert.deepEqual(formatted.change.damage, {picture: none, audio: none});
    assert.deepEqual(formatted.change.timeline, {kind:'known', occurrences:[], thumbnails:[]});
    assert.equal(digest(formatted.parked), digest(camera.parked));
    const final = await session.stats();
    assert.equal(final.audioPreparations, afterGain.audioPreparations);
    assert.ok(final.frames.bytes <= final.frames.budgetBytes);
  } finally { await session.close(); }
});
