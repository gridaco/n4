import assert from 'node:assert/strict';
import test from 'node:test';
import type { OccurrenceRanges, TimelineChange } from '../src/client/types.ts';
import { ThumbnailCache } from '../src/view/thumbnail-cache.ts';

const known = (thumbnails: OccurrenceRanges[] = []): TimelineChange => ({
  kind: 'known', occurrences: [], thumbnails,
});

test('only dirty samples of the matching occurrence are discarded, including offscreen entries', () => {
  const tiles = new ThumbnailCache<object>(64);
  const render = (path: string, frame: number) => {
    const key = path + ' ' + frame;
    const entry = tiles.begin(key, path, frame);
    const bitmap = {};
    tiles.complete(entry, bitmap);
    return {key, bitmap};
  };
  const atStart = render('shot', 10);
  const atEnd = render('shot', 24);
  const sibling = render('overlay', 10);
  const removed = render('other', 32);
  tiles.accept(known([{path: 'shot', ranges: [[10, 24]]}]), path => path !== 'other');
  assert.equal(tiles.has(atStart.key), false);
  assert.equal(tiles.has(removed.key), false);
  assert.strictEqual(tiles.get(atEnd.key), atEnd.bitmap);
  assert.strictEqual(tiles.get(sibling.key), sibling.bitmap);
  tiles.accept(known(), () => true);
  assert.strictEqual(tiles.get(sibling.key), sibling.bitmap);
  tiles.accept({kind: 'unknown'}, () => true);
  assert.equal(tiles.has(sibling.key), false);
});

test('in-flight samples survive clean revisions, and a later edit or rejection cannot publish stale pixels', () => {
  const tiles = new ThumbnailCache<string>(64);
  const clean = tiles.begin('a', 'first', 12);
  const dirty = tiles.begin('b', 'second', 12);
  const cancelled = tiles.begin('c', 'third', 12);
  tiles.accept(known(), () => true);
  tiles.accept(known([{path: 'second', ranges: [[12, 13]]}]), () => true);
  tiles.complete(clean, 'clean');
  assert.equal(tiles.get('a'), 'clean');
  const current = tiles.begin('b', 'second', 12);
  tiles.complete(dirty, 'stale');
  assert.equal(tiles.get('b'), undefined);
  tiles.complete(current, 'current');
  assert.equal(tiles.get('b'), 'current');
  tiles.complete(cancelled, null);
  assert.equal(tiles.has('c'), false);
});

test('zoom borrows the nearest resident of the same occurrence while an exact sample loads', () => {
  const tiles = new ThumbnailCache<string>(64);
  tiles.complete(tiles.begin('large', 'first', 7, 0, 16), 'coarse');
  tiles.complete(tiles.begin('small', 'first', 17, 16, 20), 'fine');
  tiles.complete(tiles.begin('repeat', 'second', 14, 12, 16), 'other placement');
  assert.equal(tiles.forDisplay('in', 'first', 14, 12, 16), 'coarse');
  assert.equal(tiles.has('in'), false);
  assert.equal(tiles.forDisplay('out', 'first', 15, 0, 32), 'fine');
  assert.equal(tiles.forDisplay('outside', 'first', 20, 20, 24), undefined);
  const exact = tiles.begin('in', 'first', 14, 12, 16);
  assert.equal(tiles.forDisplay('in', 'first', 14, 12, 16), 'coarse');
  tiles.complete(exact, 'exact');
  assert.equal(tiles.forDisplay('in', 'first', 14, 12, 16), 'exact');
  tiles.complete(tiles.begin('failed', 'first', 13, 12, 14), null);
  assert.equal(tiles.forDisplay('failed', 'first', 13, 12, 14), null);
});

test('fallback coverage follows clean ranges across edits, eviction and stale completions', () => {
  const tiles = new ThumbnailCache<string>(1);
  const add = (path: string, frame: number) => tiles.begin(path, path, frame, 0, 32);
  const pending = add('dirty', 7);
  const clean = add('first', 9);
  tiles.accept(known([
    {path: 'dirty', ranges: [[7, 8]]}, {path: 'first', ranges: [[12, 14]]},
  ]), () => true);
  tiles.complete(pending, 'stale');
  tiles.complete(clean, 'clean');
  tiles.accept(known(), () => true);
  assert.equal(tiles.forDisplay('a', 'dirty', 5, 4, 8), undefined);
  assert.equal(tiles.forDisplay('b', 'first', 5, 4, 8), 'clean');
  assert.equal(tiles.forDisplay('c', 'first', 20, 16, 24), undefined);
  assert.equal(tiles.forDisplay('d', 'first', 15, 0, 32), undefined);
  assert.equal(tiles.get('first'), 'clean');
  tiles.complete(add('second', 3), 'second');
  tiles.trim(new Map([['second', {placement: {path: 'second'}, frame: 3, start: 0, end: 32}]]));
  assert.equal(tiles.forDisplay('b', 'first', 5, 4, 8), undefined);
  tiles.clear();
  assert.equal(tiles.forDisplay('e', 'second', 5, 4, 8), undefined);
});

test('eviction preserves the visible fallback until all its replacements settle', () => {
  const tiles = new ThumbnailCache<string>(2);
  const placement = {path: 'shot'};
  tiles.complete(tiles.begin('original', 'shot', 7, 0, 16), 'coarse');
  tiles.complete(tiles.begin('offscreen', 'other', 7, 0, 16), 'unrelated');
  const wanted = new Map([
    ['left', {placement, frame: 3, start: 0, end: 8}],
    ['right', {placement, frame: 11, start: 8, end: 16}],
  ]);
  const left = tiles.begin('left', 'shot', 3, 0, 8);
  tiles.trim(wanted);
  assert.equal(tiles.has('offscreen'), false);
  assert.equal(tiles.forDisplay('right', 'shot', 11, 8, 16), 'coarse');
  tiles.complete(left, 'left');
  const right = tiles.begin('right', 'shot', 11, 8, 16);
  tiles.trim(wanted);
  assert.equal(tiles.forDisplay('right', 'shot', 11, 8, 16), 'coarse');
  tiles.complete(right, 'right');
  tiles.trim(wanted);
  assert.equal(tiles.has('original'), false);
  assert.equal(tiles.get('left'), 'left');
  assert.equal(tiles.get('right'), 'right');
});
