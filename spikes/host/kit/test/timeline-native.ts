// Run with a built headless n4 CLI to replay nested clicks against the current authors' files.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { TimelineController } from '../src/timeline/index.ts';
import type { Inspection, SeekIntent } from '../src/timeline/index.ts';

const spikes = fileURLToPath(new URL('../../../', import.meta.url));
function inspect(project: string) {
  const cli = process.env.N4_CLI ?? join(spikes, 'engine/target/debug/n4');
  const snapshot: Inspection = JSON.parse(
    execFileSync(cli, ['inspect', project], { encoding: 'utf8' }),
  );
  const intents: SeekIntent[] = [];
  const c = new TimelineController(snapshot, (intent) => intents.push(intent));
  c.resize(1200, 2000);
  return { c, intents };
}
function clickClip(c: TimelineController, path: string, clicks = 2) {
  for (const row of c.layout().rows) {
    const clip = row.clips.find((clip) => clip.placement.path === path);
    if (clip) {
      c.pointerDown((clip.x + clip.right) / 2, row.top + 14, clicks);
      return;
    }
  }
  assert.fail('Missing visible clip ' + path);
}
function expand(c: TimelineController, name: string) {
  const row = c.layout().rows.find((r) => r.label === name && r.kind === 'layer');
  assert.ok(row, name);
  c.pointerDown(10, row.top + 14);
  return row.id;
}
function clickKey(c: TimelineController, owner: string, name: string, frame: number) {
  const row = c
    .layout()
    .rows.find((r) => r.kind === 'property' && r.label === name && r.id.startsWith(owner + '/t'));
  const key = row?.keys.find((k) => k.frame === frame);
  assert.ok(row && key, 'Missing visible key ' + name + ' at ' + frame);
  c.pointerDown(key.x, row.top + 14);
}

test('Signal Atlas maps nested property keys and reused camera cuts to project time', () => {
  const project = join(spikes, 'examples/signal-atlas');
  const { c, intents } = inspect(project);
  clickClip(c, 'V1:analysis@64');
  const title = 'V1:analysis@64/V1:sounding@0';
  clickClip(c, title);
  expand(c, 'Sounder');
  const slide = expand(c, 'Instrument');
  const badge = expand(c, 'Signal lamp');
  clickKey(c, badge, 'opacity', 76);
  clickKey(c, slide, 'translate-y', 84);
  assert.deepEqual(intents.slice(-2), [
    { frame: 76, phase: 'exact' }, { frame: 84, phase: 'exact' },
  ]);
  assert.equal(c.layout().scopePath, title);

  const second = inspect(project);
  clickClip(second.c, 'V1:survey@0');
  const nextUse = second.c.layout().buttons.find(b => b.action === 'nextUse');
  assert.ok(nextUse);
  second.c.pointerDown(nextUse.x + 3, 12);
  assert.equal(second.c.layout().scopePath, 'V1:survey@128');
  clickClip(second.c, 'V1:survey@128/CAM:detail@12', 1);
  assert.deepEqual(second.intents.at(-1), { frame: 140, phase: 'exact' });
});
