import assert from 'node:assert/strict';
import test from 'node:test';
import { formatElapsedTime, timelineTicks } from '../src/timeline/index.ts';

test('elapsed time uses rational FPS and carries minutes and hours', () => {
  assert.equal(formatElapsedTime(397, [30, 1]), '0:13.233');
  assert.equal(formatElapsedTime(1451, [24000, 1001]), '1:00.519');
  assert.equal(formatElapsedTime(108_090, [30, 1]), '1:00:03.000');
});

test('the ruler ticks in local elapsed seconds even on a fractional frame clock', () => {
  const ticks = timelineTicks([100, 172], 100, [24000, 1001], 320);
  assert.deepEqual(ticks.map(tick => tick.label), ['0:00', '0:01', '0:02', '0:03']);
  assert.equal(ticks[0]!.frame, 100);
  assert.equal(ticks[1]!.frame, 100 + 24000 / 1001);
  assert.equal(ticks[3]!.frame < 172, true);

  assert.deepEqual(timelineTicks([224, 284], 200, [30, 1], 240), [
    { frame: 230, label: '0:01' },
    { frame: 260, label: '0:02' },
  ]);
});

test('deep zoom labels individual frame boundaries as elapsed time', () => {
  assert.deepEqual(timelineTicks([100.25, 103.75], 100, [24000, 1001], 600), [
    { frame: 101, label: '0:00.042' },
    { frame: 102, label: '0:00.083' },
    { frame: 103, label: '0:00.125' },
  ]);
});
