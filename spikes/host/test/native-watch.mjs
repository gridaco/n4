// Exercise protocol ownership directly. This client only reads and writes pipes.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

test('idle native service signals saved inputs and reissues the hint after a racing export', { timeout: 30_000 }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'n4-wire-watch-'));
  const detail = join(root, 'sequences', 'picture.xml');
  await mkdir(join(root, 'sequences'));
  await writeFile(join(root, 'project.proj'), 'project trial fps 30/1 size 320x180\n[0,600) V1 picture\n');
  await writeFile(detail, '<sequence background="#ffffff"/>');
  const bin = process.env.N4_HOST_BIN || fileURLToPath(new URL('../../engine/target/debug/n4-host', import.meta.url));
  const child = spawn(bin, [root], { stdio: ['pipe', 'pipe', 'pipe', 'pipe'] });
  const closed = once(child, 'close');
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  let diagnostics = '';
  child.stderr.setEncoding('utf8').on('data', data => { diagnostics += data; });
  child.stdio[3].resume();
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await closed;
    await rm(root, { recursive: true, force: true });
  });
  async function read() {
    const line = await lines.next();
    assert.ok(!line.done, diagnostics);
    return JSON.parse(line.value);
  }
  let id = 0;
  async function command(op, args = {}) {
    child.stdin.write(JSON.stringify({ id: ++id, op, ...args }) + '\n');
    const reply = await read();
    assert.equal(reply.id, id);
    assert.equal('inputs' in reply, false, 'Input manifests belong to the engine');
    return reply;
  }

  const initial = await command('inspect');
  assert.equal(initial.protocol, 4);
  assert.equal(initial.revision, 0);
  await writeFile(detail, '<sequence background="#00ff00"/>');
  // No command, registration or status polling between the save and the notice.
  const hint = { event: 'inputsChanged', revision: 0 };
  assert.deepEqual(await read(), hint);

  // A host may start exporting the accepted revision just as a notice arrives.
  const started = await command('export', { revision: 0, path: join(root, 'trial.mp4') });
  assert.equal(started.status.state, 'running');
  const busy = await command('reopen', { revision: 0, frame: 9 });
  assert.equal(busy.ok, false);
  assert.equal(busy.error, 'export in progress');
  await command('cancelexport');
  // Completion must rearm the notice even if the host never polls export status.
  assert.deepEqual(await read(), hint);
  const accepted = await command('reopen', { revision: 0, frame: 9 });
  assert.equal(accepted.ok, true);
  assert.equal(accepted.frame, 9);
  const picture = [{path:'V1:picture@0', ranges:[[0,600]]}];
  assert.deepEqual(accepted.change, {fromRevision:0, toRevision:1, damage:{picture:{kind:'all'}, audio:{kind:'none'}},
    timeline:{kind:'known', occurrences:picture, thumbnails:picture}});
  await command('close');
  assert.deepEqual(await closed, [0, null]);
});
