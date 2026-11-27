import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LiveReload, NativeSession } from './kit/src/client/index.ts';

const [project, requestedFrame = '0', ...extra] = process.argv.slice(2);
if (!project || extra.length || !/^\d+$/.test(requestedFrame) || !Number.isSafeInteger(Number(requestedFrame))) {
  console.error('Usage: just watch-project <project> [parked-frame]');
  process.exit(2);
}
const executable = process.env.N4_HOST_BIN || fileURLToPath(new URL('../engine/target/debug/n4-host', import.meta.url));
const session = await NativeSession.open(executable, resolve(project));
let cursor = Math.min(Number(requestedFrame), session.document.snapshot.duration - 1);
let stopping = false;
function emit(event) { console.log(JSON.stringify(event)); }
function show(event, picture, change) {
  cursor = picture.frame;
  emit({ event, revision: picture.revision, frame: cursor, change,
    rgbaSha256: createHash('sha256').update(picture.rgba).digest('hex') });
}
const reload = new LiveReload(async () => {
  const { parked, change } = await session.reopen(cursor);
  show('refreshed', parked, change);
}, { onError(error) {
  if (!stopping) emit({ event: 'rejected', revision: session.document.revision, frame: cursor, error: String(error.message ?? error) });
} });
function stop() {
  stopping = true;
  reload.dispose();
  // Terminal signals can stop the native child before it acknowledges close.
  void session.close().catch(() => {});
}
process.once('SIGINT', stop);
process.once('SIGTERM', stop);
try {
  show('ready', await session.seek(cursor, 1));
  session.onProjectChange(() => reload.changed());
  if (process.stderr.isTTY) {
    console.error(`Watching ${project} for saved changes. Press Ctrl+C to stop.`);
  }
  await session.closed;
  if (!stopping) throw new Error('Native session stopped');
} catch (error) {
  if (!stopping) throw error;
} finally {
  reload.dispose();
  if (stopping) await session.closed;
  else await session.close();
}
