import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { NativeSession } from '../src/client/index.ts';

// A process fixture exercises the actual control/pixel pipes, including EOF during an image.
async function fake(body: string, run: (script: string) => Promise<void>) {
  const dir = await mkdtemp(join(tmpdir(), 'n4-wire-'));
  try {
    const script = join(dir, 'service.mjs');
    await writeFile(script, `#!${process.execPath}
      import { createInterface } from 'node:readline';
      import { writeSync } from 'node:fs';
      const doc={sessionId:'test',revision:0,size:[1,1],protocol:4,peaks:[],
        snapshot:{name:'fixture',duration:1,fps:[24,1],placements:[],definitions:{}}};
      for await (const line of createInterface({ input:process.stdin })) {
        const {op,id,frame,generation}=JSON.parse(line);
        function send(fields) { console.log(JSON.stringify({ id,ok:true,...fields })); }
        ${body}
      }
    `, { mode: 0o700 });
    await run(script);
  } finally { await rm(dir, { recursive:true, force:true }); }
}
test('malformed host inspection fails before opening', async () => {
  await fake("doc.snapshot.placements.push({name:'missing ranges'});send(doc)", async path => {
    await assert.rejects(NativeSession.open(process.execPath,path), /inspection/);
  });
});
test('truncated payload rejects a pending seek and releases the worker', async () => {
  await fake(`if(op==='inspect')send(doc);
    if(op==='seek'){send({revision:0,frame,generation,width:1,height:1,bytes:4});writeSync(3,Buffer.alloc(2));process.exit(0);}`, async path => {
    const s = await NativeSession.open(process.execPath,path);
    await assert.rejects(s.seek(0), /Truncated|closed/);
    await s.closed;
  });
});

test('native change notices survive late subscription and interleave with ordinary replies', async () => {
  await fake(`
    if (op === 'inspect') {
      console.log(JSON.stringify({event:'inputsChanged',revision:0}));
      send(doc);
    }
    if (op === 'close') { send({}); break; }
  `, async path => {
    const session = await NativeSession.open(process.execPath, path);
    try {
      let calls = 0;
      const unsubscribe = session.onProjectChange(() => { calls++; });
      assert.equal(calls, 1);
      assert.equal((await session.inspect()).revision, 0);
      assert.equal(calls, 2);
      unsubscribe();
      await session.inspect();
      await session.inspect();
      assert.equal(calls, 2);
      session.onProjectChange(() => { calls++; });
      assert.equal(calls, 3);
    } finally { await session.close(); }
  });
});

test('an invalid damage interval cannot advance the accepted revision', async () => {
  await fake(`
    if (op === 'inspect') send(doc);
    if (op === 'reopen') {
      send({...doc, revision:1, frame:0, generation:0, width:1, height:1, bytes:4,
        change:{fromRevision:0,toRevision:1,timeline:{kind:'unknown'},damage:{picture:{kind:'ranges',ranges:[[4,8],[6,9]]},audio:{kind:'none'}}}});
      writeSync(3,Buffer.alloc(4));
    }
    if (op === 'close') { send({}); break; }
  `, async path => {
    const session = await NativeSession.open(process.execPath, path);
    try {
      await assert.rejects(session.reopen(), /revision change/);
      assert.equal(session.document.revision, 0);
    } finally { await session.close(); }
  });
});

test('occurrence evidence must address a unique current row within its half-open interval', async () => {
  await fake(`
    const path = 'V1:opening@1';
    const audio = 'A1:voice@1';
    doc.snapshot.duration = 4;
    doc.snapshot.placements = [path, audio].map((path,i) => ({
      name:i?'voice':'opening', path, track:i?'A1':'V1', local:[1,4], project:[1,4]
    }));
    if (op === 'inspect') send(doc);
    if (op === 'reopen') {
      const entry = {path,ranges:[[1,4]]};
      const examples = [[{...entry,path:'V1:missing@0'}], [{...entry,ranges:[[0,4]]}],
        [entry,entry], [{...entry,path:audio}], [entry]];
      send({...doc,revision:1,frame:1,generation:0,width:1,height:1,bytes:4,
        change:{fromRevision:0,toRevision:1,damage:{picture:{kind:'all'},audio:{kind:'none'}},
        timeline:{kind:'known',occurrences:[entry],thumbnails:examples[frame]}}});
      writeSync(3,Buffer.alloc(4));
    }
    if (op === 'close') { send({}); break; }
  `, async path => {
    const session = await NativeSession.open(process.execPath, path);
    try {
      for (const fixture of [0,1,2,3]) {
        await assert.rejects(session.reopen(fixture), /revision change/);
        assert.equal(session.document.revision, 0);
      }
      assert.equal((await session.reopen(4)).revision, 1);
    } finally { await session.close(); }
  });
});

test('editable host validates transition state and HUD bounds before presenting pixels', async () => {
  await fake(`
    doc.editable = process.argv[2] === '--editor';
    if (op === 'inspect') send(doc);
    if (op === 'editor') {
      const state = {sessionId:doc.sessionId, revision:0, hudVersion:3, frame:0, scope:'root',
        selection:null, selections:[], selectionSource:null, hud:null, marquee:null, hover:null,
        tracks:[], dirty:false, conflict:false,
        canUndo:false, canRedo:false};
      const change = {fromRevision:0,toRevision:1,timeline:{kind:'unknown'},
        damage:{picture:{kind:'all'},audio:{kind:'none'}}};
      send({...doc,revision:1,frame:0,generation:0,width:1,height:1,bytes:4,change,
        result:{state,change}});
      writeSync(3,Buffer.alloc(4));
    }
    if (op === 'hud') {
      send({revision:0,frame,generation:0,width:1,height:1,bytes:4,x:1,y:0,hudVersion:3});
      writeSync(3,Buffer.alloc(4));
    }
    if (op === 'close') { send({}); break; }
  `, async path => {
    const session = await NativeSession.open(path, 'fixture', undefined, {editor:true});
    try {
      await assert.rejects(session.editor({op:'undo',revision:0}), /editor result/);
      assert.equal(session.document.revision, 0);
      await assert.rejects(session.hud(0, 0), /mismatched HUD tile/);
    } finally { await session.close(); }
  });
});
