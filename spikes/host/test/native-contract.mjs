// Run explicitly on a machine with the native engine, Metal and FFmpeg installed.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { NativeSession } from '@n4/host-kit/client';

const spikes = fileURLToPath(new URL('../../', import.meta.url));
const bin = process.env.N4_HOST_BIN || join(spikes, 'engine/target/debug/n4-host');
const cli = process.env.N4_CLI || join(spikes, 'engine/target/debug/n4');
const temp = mkdtempSync(join(tmpdir(), 'n4-host-contract-'));
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

async function compare(project, frame, checkReload) {
  const session = await NativeSession.open(bin, project);
  try {
    const reference = join(temp, 'ref-' + frame);
    execFileSync(cli, ['render', project, '--frames', String(frame), '--out', reference]);
    const png = readdirSync(reference).find(name => name.endsWith('.png'));
    const expected = execFileSync('ffmpeg', [
      '-v', 'error', '-i', join(reference, png), '-pix_fmt', 'rgba', '-f', 'rawvideo', 'pipe:1'
    ], { maxBuffer: 40 * 1024 * 1024 });
    const picture = await session.seek(frame, 1);
    assert.equal(picture.frame, frame);
    assert.equal(picture.rgba.byteLength, picture.width * picture.height * 4);
    assert.equal(digest(picture.rgba), digest(expected));
    if (checkReload) {
      const burst = await Promise.allSettled(
        [frame - 2, frame + 2, frame].map((target, i) => session.seek(target, i + 2))
      );
      assert.equal(burst.at(-1).status, 'fulfilled');
      assert.equal(digest(burst.at(-1).value.rgba), digest(expected));
      await assert.rejects(session.seek(frame + 2, 2), /stale generation/);
      const thumbnail = await session.thumbnail(frame, 4);
      assert.ok(thumbnail.width > 0 && thumbnail.width < picture.width);
      assert.ok(session.document.peaks.length > 0);
      const file = join(project, 'sequences/survey.xml');
      const original = readFileSync(file, 'utf8');
      const revision = session.document.revision;
      writeFileSync(file, '<sequence>');
      await assert.rejects(session.reopen());
      assert.equal(session.document.revision, revision);
      assert.equal(digest((await session.seek(frame, 5)).rgba), digest(expected));
      const changed = original.replace('background="#eeebde"', 'background="#126547"');
      assert.notEqual(changed, original);
      writeFileSync(file, changed);
      const {parked} = await session.reopen(frame);
      assert.ok(session.document.revision > revision);
      assert.equal(parked.frame, frame);
      assert.equal(parked.revision, session.document.revision);
      const updated = await session.seek(frame, 6);
      assert.equal(updated.revision, session.document.revision);
      assert.notEqual(digest(updated.rgba), digest(expected));
      assert.equal(digest(parked.rgba), digest(updated.rgba));
    }
    console.log(JSON.stringify({project: session.document.snapshot.name, frame,
      size: [picture.width, picture.height], rgbaSha256: digest(expected), checkedReload: checkReload}));
  } finally {
    await session.close();
    await session.close();
  }
}

try {
  const project = join(temp, 'signal-atlas');
  cpSync(join(spikes, 'examples/signal-atlas'), project, { recursive: true });
  await compare(project, 42, true);
  if (process.env.N4_TEST_OPENUSD === '1') {
    await compare(join(spikes, 'examples/field-unit'), 70, false);
  }
} finally {
  rmSync(temp, {recursive: true, force: true});
}
