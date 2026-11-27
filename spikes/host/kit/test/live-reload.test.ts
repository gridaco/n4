import assert from 'node:assert/strict';
import test from 'node:test';
import { setImmediate } from 'node:timers/promises';
import { continuesSession, LiveReload } from '../src/client/live-reload.ts';

test('only consecutive revisions from the same native session can carry cached data', () => {
  const before = {sessionId: 'first', revision: 0};
  const after = {sessionId: 'first', revision: 1};
  const change = {fromRevision: 0, toRevision: 1};
  assert.ok(continuesSession(before, after, change));
  assert.equal(continuesSession(before, {...after, sessionId: 'restarted'}, change), false);
  assert.equal(continuesSession(before, {...after, revision: 2}, {...change, toRevision: 2}), false);
});

function rig() {
  const attempts: ReturnType<typeof Promise.withResolvers<number>>[] = [];
  const busy: boolean[] = [];
  const errors: unknown[] = [];
  const reload = new LiveReload(() => {
    const attempt = Promise.withResolvers<number>();
    attempts.push(attempt);
    return attempt.promise;
  }, { onBusy: value => busy.push(value), onError: error => errors.push(error) });
  return { reload, attempts, busy, errors };
}

test('manual and external requests share a load, then read once more if a save landed mid-load', async () => {
  const { reload, attempts, busy, errors } = rig();
  reload.changed();
  const done = reload.refresh();
  reload.changed();
  await setImmediate();
  assert.equal(attempts.length, 1);
  reload.changed();
  reload.changed();
  attempts[0]!.reject(new Error('intermediate file'));
  await setImmediate();
  assert.equal(attempts.length, 2);
  attempts[1]!.resolve(2);
  assert.equal(await done, 2);
  assert.deepEqual(busy, [true, false]);
  assert.deepEqual(errors, []);
});

test('a rejected save reports once, a repair retries, and disposal discards queued work', async () => {
  const { reload, attempts, errors } = rig();
  const invalid = new Error('invalid edit');
  reload.changed();
  await setImmediate();
  attempts[0]!.reject(invalid);
  await setImmediate();
  assert.deepEqual(errors, [invalid]);
  assert.equal(reload.busy, false);
  const repaired = reload.refresh();
  await setImmediate();
  reload.changed();
  reload.dispose();
  attempts[1]!.resolve(2);
  assert.equal(await repaired, 2);
  reload.changed();
  assert.equal(attempts.length, 2);
  await assert.rejects(reload.refresh(), /disposed/);
});
