import assert from 'node:assert/strict';
import test from 'node:test';
import { TimelineController, type Placement } from '../src/timeline/index.ts';
import { planThumbnails } from '../src/timeline/thumbnails.ts';
import { ThumbnailCache } from '../src/view/thumbnail-cache.ts';

function clip(path: string, start: number, end: number): Placement {
  return { path, name: path, track: 'V1', local: [0, end - start], project: [start, end] };
}

function timeline(placements: Placement[], start: number) {
  const c = new TimelineController(
    { name: 'Film', duration: 800, fps: [30, 1], placements, definitions: {} },
    () => {},
  );
  c.resize(970, 300); // 760 px of timeline, 80 frames visible.
  c.zoom(0.1, 0);
  c.pan(start * 9.5);
  return c;
}

test('pan reuses warm samples, loads one entering tile and reuses it on return', () => {
  const c = timeline([clip('shot', 5, 750)], 102);
  const cache = new ThumbnailCache<boolean>(64);
  const loadMissing = () => {
    const { wanted } = planThumbnails(c.layout());
    let loads = 0;
    for (const [key, { placement, frame }] of wanted) {
      if (cache.has(key)) continue;
      cache.complete(cache.begin(key, placement.path, frame), true);
      loads++;
    }
    cache.trim(wanted);
    return loads;
  };
  assert.equal(loadMissing(), 11);
  c.pan(9.5);
  assert.equal(loadMissing(), 0);
  c.pan(8 * 9.5);
  assert.equal(loadMissing(), 1);
  c.pan(-9 * 9.5);
  assert.deepEqual(c.layout().visible, [102, 182]);
  assert.equal(loadMissing(), 0);
});

test('viewport crops keep full tile bounds and a one-frame shot gets its own sample', () => {
  const c = timeline([clip('head', 5, 133), clip('insert', 133, 134), clip('tail', 134, 800)], 132);
  const { tiles, wanted } = planThumbnails(c.layout());
  const head = tiles.get('head')![0]!;
  const insert = tiles.get('insert')![0]!;
  const tail = tiles.get('tail')!.at(-1)!;
  assert.deepEqual([head.start, head.end], [125, 133]);
  assert.ok(wanted.get(head.key)!.frame < c.layout().visible[0]);
  assert.deepEqual([insert.start, insert.end, wanted.get(insert.key)!.frame], [133, 134, 133]);
  assert.deepEqual([tail.start, tail.end], [206, 214]);
  c.pan(9.5);
  assert.equal(planThumbnails(c.layout()).tiles.has('head'), false);
});

test('small zoom changes retain sample identities until the grid changes density', () => {
  const c = timeline([clip('shot', 5, 750)], 0);
  const first = planThumbnails(c.layout());
  c.zoom(1.05, 0);
  const nearby = planThumbnails(c.layout());
  assert.deepEqual([...nearby.wanted.keys()], [...first.wanted.keys()]);
  c.zoom(0.5, 0);
  const closer = planThumbnails(c.layout()).tiles.get('shot')![0]!;
  const previous = nearby.tiles.get('shot')![0]!;
  assert.deepEqual([previous.start, previous.end], [5, 13]);
  assert.deepEqual([closer.start, closer.end], [5, 9]);
});
