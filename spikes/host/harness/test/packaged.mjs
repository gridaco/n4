// Exercise the installed macOS harness from outside the checkout over a private CDP pipe.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { setTimeout as wait } from 'node:timers/promises';

const [bundle, project, target = '0'] = process.argv.slice(2);
if (!bundle || !project) throw Error('usage: node packaged.mjs bundle.app project [frame]');
const profile = mkdtempSync(join(tmpdir(), 'n4-embedded-'));
const environment = { ...process.env, PATH: '/usr/bin:/bin' };
delete environment.N4_HOST_BIN;
const relocated = join(profile, basename(bundle));
execFileSync('/usr/bin/ditto', [resolve(bundle), relocated]);
const child = spawn(join(relocated, 'Contents/MacOS/N4Viewer'),
  ['--remote-debugging-pipe', '--user-data-dir=' + join(profile, 'profile'), resolve(project), '--frame=' + target],
  { cwd: tmpdir(), stdio: ['ignore','ignore','pipe','pipe','pipe'],
    env: environment });
let lastPageState = '';
let errors = '', buffer = '', serial = 0, sessionId;
const pending = new Map();
child.stderr.on('data', b => { errors = (errors + b).slice(-8000); });
child.stdio[4].setEncoding('utf8').on('data', s => {
  buffer += s;
  let split;
  while ((split = buffer.indexOf('\0')) >= 0) {
    const msg = JSON.parse(buffer.slice(0, split));
    buffer = buffer.slice(split + 1);
    const waiter = pending.get(msg.id);
    if (waiter) {
      pending.delete(msg.id);
      if (msg.error) waiter.reject(new Error(msg.error.message));
      else waiter.resolve(msg.result);
    }
  }
});
const exited = once(child,'exit');
function cdp(method, params = {}, session = sessionId) {
  const id = ++serial;
  return new Promise((resolve, reject) => {
    pending.set(id, {resolve, reject});
    child.stdio[3].write(JSON.stringify({id,method,params,sessionId:session}) + '\0');
  });
}
async function evaluate(expression, targetSession = sessionId) {
  const res = await cdp('Runtime.evaluate', { expression, awaitPromise:true, returnByValue:true }, targetSession);
  if (res.exceptionDetails) throw Error(res.exceptionDetails.text);
  return res.result.value;
}
async function until(fn) {
  const start = Date.now();
  while (Date.now() - start < 45000) {

    const value = await fn();
    if (value) return value;
    await wait(80);
  }
  throw Error('Timed out in ' + fn.toString() + ': ' + lastPageState + ' ' + errors);
}
const timer = setTimeout(() => { child.kill('SIGKILL'); console.error('Harness deadline: ' + errors); process.exit(1); }, 100000);
const shown = `document.querySelector('[data-role=preview]')?.dataset.frame === '${Number(target)}'`;
async function leaveSkimmer() {
  // The window can open with the physical pointer already over the timeline.
  if (await evaluate("document.querySelector('[data-role=skimmer]')?.hidden === false")) {
    await cdp('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 8, y: 75 });
  }
}
function sidecars() {
  const rows = execFileSync('/bin/ps',['-A','-o','pid=,ppid=,comm='],{encoding:'utf8'}).split('\n');
  return rows.flatMap(line => {
    const fields = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
    return fields && Number(fields[2]) === child.pid && fields[3].endsWith('/n4-host') ? [Number(fields[1])] : [];
  });
}
try {
  const page = await until(async () => (await cdp('Target.getTargets')).targetInfos.find(t=>t.type==='page'));
  sessionId = (await cdp('Target.attachToTarget',{targetId:page.targetId,flatten:true})).sessionId;
  await until(async () => {
    await leaveSkimmer();
    const state = await evaluate("({url:location.href,text:document.body?.innerText,frame:document.querySelector('[data-role=preview]')?.dataset.frame})");
    lastPageState = JSON.stringify(state);
    return state.frame === target;
  });
  await evaluate("document.querySelector('[data-role=export]').click()");
  const exportPage = await until(async () => (await cdp('Target.getTargets')).targetInfos
    .find(target => target.type === 'page' && target.url.endsWith('/export.html')));
  const exportSession = (await cdp('Target.attachToTarget', {
    targetId: exportPage.targetId, flatten: true,
  })).sessionId;
  await until(() => evaluate(
    "!!document.querySelector('[data-role=start-export]:enabled')",
    exportSession,
  ));
  await cdp('Target.closeTarget', { targetId: exportPage.targetId });
  await until(async () => !(await cdp('Target.getTargets')).targetInfos
    .some(target => target.targetId === exportPage.targetId));
  const first = sidecars();
  assert.equal(first.length,1);
  await cdp('Page.reload');
  await until(async () => {
    try {
      const p = sidecars();
      if (p.length !== 1 || p[0] === first[0]) return false;
      await leaveSkimmer();
      return await evaluate(shown);
    }
    catch { return false; }
  });
  const second = sidecars()[0];
  process.kill(second, 'SIGKILL');
  await until(()=>sidecars().length === 0);
  await evaluate("document.querySelector('[data-role=reopen]').click()");
  await until(async () => sidecars().length === 1 && await evaluate(shown + " && Number(document.querySelector('[data-role=preview]').dataset.revision) > 0"));
  const finalPid = sidecars()[0];
  await cdp('Target.closeTarget',{targetId:page.targetId});
  await exited;
  await until(() => { try { process.kill(finalPid,0); return false; } catch { return true; } });
  console.log(JSON.stringify({ frame:Number(target), relocated:true,
    exportWindow:true, reopened:true, recoveredService:true, sidecarReleased:true }));
} finally {
  clearTimeout(timer);
  if (child.exitCode === null && child.signalCode === null) child.kill();
  await exited;
  rmSync(profile, {recursive:true, force:true});
}
