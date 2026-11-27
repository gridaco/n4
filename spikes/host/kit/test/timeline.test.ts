import assert from 'node:assert/strict';
import test from 'node:test';
import { TimelineController, bodyTop, rowHeight, trackHeight } from '../src/timeline/index.ts';
import type { Inspection, SeekIntent } from '../src/timeline/index.ts';
import type { EditorState, EditorTarget, RevisionChange, TimelineChange } from '../src/client/types.ts';

const doc: Inspection = {
  name: 'Edit',
  fps: [24000, 1001],
  duration: 150,
  placements: [
    {
      path: 'V1:card@0',
      name: 'card',
      track: 'V1',
      definition: 'card',
      local: [0, 50],
      project: [0, 50],
    },
    {
      path: 'V1:card@100',
      name: 'card',
      track: 'V1',
      definition: 'card',
      local: [100, 150],
      project: [100, 150],
    },
  ],
  definitions: {
    card: {
      layers: [
        {
          name: 'Title',
          kind: 'text',
          start: 5,
          end: 25,
          children: [],
          tracks: [
            {
              name: 'opacity',
              keys: [
                { frame: 0, value: 0 },
                { frame: 4, value: 1 },
              ],
            },
          ],
        },
      ],
    },
  },
};

test('projects the native pick and track state through the timeline without accepting local edits', () => {
  const snapshot = structuredClone(doc);
  const card = snapshot.definitions.card.layers[0];
  snapshot.definitions.card.layers = [
    { ...card, name: 'Camera', kind: 'camera', tracks: [] },
    {
      ...card, name: 'Group', kind: 'transform-group', authoredPath: [0], tracks: [],
      children: [{ ...card, authoredPath: [0, 0] }],
    },
  ];
  snapshot.definitions.overlay = {
    layers: [{ ...card, name: 'Badge', kind: 'rect', authoredPath: [0], tracks: [] }],
  };
  snapshot.placements.push({
    path: 'V2:overlay@0', name: 'overlay', track: 'V2', definition: 'overlay',
    local: [0, 150], project: [0, 150], enabled: false,
  });
  const selected: EditorTarget = { kind: 'layer', occurrence: 'V1:card@100', layers: [0, 0] };
  const badge: EditorTarget = { kind: 'layer', occurrence: 'V2:overlay@0', layers: [0] };
  const state: EditorState = {
    sessionId: 'session', revision: 3, hudVersion: 4, frame: 111, scope: 'root',
    selection: selected, selections: [selected], selectionSource: null, hud: null, marquee: null, hover: null,
    tracks: [{ scope: 'root', track: 'V2', enabled: false, editable: true }],
    dirty: true, conflict: false, canUndo: true, canRedo: false,
  };
  const selections: EditorTarget[] = [];
  const scopes: string[] = [];
  const trackChanges: [string, string, boolean][] = [];
  const c = new TimelineController(snapshot, () => {}, undefined, {
    select: target => selections.push(target),
    enterScope: scope => scopes.push(scope),
    setTrackEnabled: (...request) => trackChanges.push(request),
  });
  c.resize(800, 360);
  c.setEditorState(state);
  const root = c.layout();
  assert.deepEqual(root.rows[0].clips.map(clip => clip.selected), [false, true]);
  const picked = root.rows.find(row => row.selected)!;
  assert.deepEqual([picked.label, picked.range], ['Group › Title', [105, 125]]);
  c.selectRow(picked.id);
  assert.deepEqual(selections, [selected]);
  const first = root.rows[0].clips[0];
  c.pointerDown((first.right + first.x) / 2, root.rows[0].top + 4);
  assert.deepEqual(selections.at(-1), { kind: 'placement', occurrence: 'V1:card@0' });
  assert.equal(c.layout().rows[0].clips[1].selected, true);
  c.toggleTrack('lane/V2');
  assert.deepEqual(trackChanges, [['root', 'V2', true]]);
  assert.equal(c.layout().rows.find(row => row.id === 'lane/V2')?.track?.enabled, false);
  c.setEditorState({ ...state, hudVersion: 5, selections: [selected, badge] });
  const multi = c.layout();
  assert.deepEqual(multi.rows.flatMap(row => row.clips.filter(clip => clip.selected).map(clip => clip.placement.path)),
    ['V1:card@100', 'V2:overlay@0']);
  assert.deepEqual(multi.rows.filter(row => row.selected).map(row => row.target), [selected, badge]);
  assert.equal(new Set(multi.rows.map(row => row.id)).size, multi.rows.length);
  c.enter('V1:card@100');
  assert.deepEqual(scopes, ['V1:card@100']);
  assert.equal(c.layout().scopePath, '');
  c.setEditorState({ ...state, hudVersion: 6, scope: 'V1:card@100', tracks: [] });
  assert.equal(c.layout().rows.find(row => row.id === 'layer/1/0')?.selected, true);
});

test('accepted change marks name the two uses, preserve gaps and expose a one-frame cut when zoomed out', () => {
  const card0 = 'V1:card@0', card1 = 'V1:card@100', child = card1 + '/CAM:detail@1';
  const snapshot = structuredClone(doc);
  snapshot.placements[1]!.children = [{path: child, name: 'detail', track: 'CAM',
    local: [1, 12], project: [101, 112]}];
  snapshot.placements.push({path:'V2:overlay@0', name:'overlay', track:'V2',
    local:[0,150], project:[0,150]});
  const seeks: SeekIntent[] = [];
  const c = new TimelineController(snapshot, seek => seeks.push(seek));
  c.resize(200, 300);
  const change = (timeline: TimelineChange): RevisionChange => ({fromRevision: 1, toRevision: 2,
    damage: {picture: {kind: 'ranges', ranges: [[10, 12], [101, 102]]}, audio: {kind: 'none'}}, timeline});
  c.replace(snapshot, change({kind:'known', thumbnails:[], occurrences:[
    {path:card0, ranges:[[10,12]]}, {path:child, ranges:[[101,102]]},
  ]}));
  const [first, second] = c.layout().rows[0]!.clips;
  assert.deepEqual(first!.changedRanges, [[10,12]]);
  assert.deepEqual(second!.changedRanges, [[101,102]]);
  assert.deepEqual(c.layout().rows[1]!.clips[0]!.changedRanges, []);
  assert.deepEqual(c.layout().rulerChanges, []);
  const [left, right] = c.markBounds([101,102], second)!;
  assert.equal(right - left, 3);
  const x = right - 0.1, y = bodyTop + trackHeight - 5;
  assert.equal(c.hitTest(x, y)?.kind, 'clip');
  c.pointerDown(x, y);
  assert.deepEqual(seeks.at(-1), {frame: c.frameAt(x), phase: 'exact'});
  assert.deepEqual(c.selection, {id: card1});
  c.enter(card1);
  assert.deepEqual(c.layout().rulerChanges, [[101,102]]);
  assert.deepEqual(c.layout().rows[0]!.clips[0]!.changedRanges, [[101,102]]);
  c.replace(snapshot, change({kind: 'known', occurrences: [], thumbnails: []}));
  assert.deepEqual(c.layout().rulerChanges, []);
  assert.deepEqual(c.layout().rows[0]!.clips[0]!.changedRanges, []);
  c.up();
  c.replace(snapshot, change({kind: 'unknown'}));
  assert.deepEqual(c.layout().rulerChanges, [[10,12],[101,102]]);
  assert.ok(c.layout().rows.flatMap(row => row.clips).every(clip => !clip.changedRanges.length));
});

test('selection, shared definitions and local keys follow the selected occurrence', () => {
  const seeks: SeekIntent[] = [];
  const c = new TimelineController(doc, (intent) => seeks.push(intent));
  c.resize(900, 300);
  const clip = c.layout().rows[0].clips[1];
  c.pointerDown((clip.x + clip.right) / 2, 64, 2);
  assert.equal(c.layout().scopePath, clip.placement.path);
  assert.deepEqual(c.layout().rows[0].range, [105, 125]);
  c.pointerDown(10, c.layout().rows[0].top + 10);
  const row = c.layout().rows[1];
  assert.deepEqual(
    row.keys.map((k) => k.frame),
    [105, 109],
  );
  c.pointerDown(row.keys[1].x, row.top + 10);
  assert.deepEqual(seeks.at(-1), { frame: 109, phase: 'exact' });
  const use = c.layout().buttons.find((b) => b.action === 'nextUse')!;
  c.pointerDown(use.x + 1, 10);
  assert.equal(c.layout().scopePath, 'V1:card@0');
  assert.deepEqual(seeks.at(-1), { frame: 9, phase: 'exact' });
});

test('drag keeps an immediate cursor and commits a clamped exact release', () => {
  const events: SeekIntent[] = [];
  const c = new TimelineController(doc, (event) => events.push(event));
  c.resize(810, 250);
  c.pointerDown(c.frameToX(20) + 0.5, 42);
  c.pointerMove(c.frameToX(31) + 0.5);
  c.setPlaybackFrame(10);
  assert.equal(c.cursor, 31);
  c.pointerMove(c.frameToX(31) + 0.5);
  c.pointerUp(1000);
  assert.deepEqual(events, [
    { frame: 20, phase: 'scrub' },
    { frame: 31, phase: 'scrub' },
    { frame: 149, phase: 'exact' },
  ]);
  c.keyboard('ArrowRight');
  assert.equal(c.cursor, 149);
  c.keyboard('Home');
  assert.equal(c.cursor, 0);
});

test('zoom, pan and reopened occurrences keep coordinates in range', () => {
  const c = new TimelineController(doc, () => {});
  c.resize(800, 200);
  c.enter('V1:card@100');
  c.zoom(0.5, 125);
  assert.equal(c.layout().visible[1] - c.layout().visible[0], 25);
  c.pan(10000);
  assert.deepEqual(c.layout().visible, [125, 150]);
  assert.equal(c.frameAt(c.frameToX(130) + 0.2), 130);
  c.replace({ ...doc, placements: [doc.placements[0]] });
  assert.equal(c.layout().scopePath, '');
  assert.deepEqual(c.layout().visible, [0, 150]);
});

test('repeated pointer zoom keeps the exact time under the pointer', () => {
  const c = new TimelineController(doc, () => {});
  const anchor = 91.75;
  const x = c.frameToX(anchor);
  for (let i = 0; i < 20; i++) {
    c.zoomAt(0.87, x);
    assert.ok(Math.abs(c.frameToX(anchor) - x) < 1e-7);
  }
  c.zoomAt(0.01, x);
  assert.ok(Math.abs(c.frameToX(anchor) - x) < 1e-7);
  const { gutter, width } = c.layout();
  assert.equal(c.frameAt(gutter), 91);
  assert.equal(c.frameAt(width), 92);
  c.pan(0.25);
  assert.ok(Math.abs(c.frameToX(anchor) - (x - 0.25)) < 1e-7);

  c.fit();
  const boundary = c.frameToX(83);
  for (let i = 0; i < 20; i++) {
    c.zoomAt(0.87, boundary);
    assert.equal(c.frameAt(boundary), 83);
  }
});

test('refresh preserves inspection at a surviving occurrence and drops a removed key', () => {
  const c = new TimelineController(doc, () => {});
  c.enter('V1:card@100');
  c.selectRow('layer/0');
  c.zoom(0.5, 112);
  const visible = c.layout().visible;
  const opacity = c.layout().rows[1]!;
  c.pointerDown(opacity.keys.find(k => k.frame === 109)!.x, opacity.top + 5);
  assert.deepEqual(c.selection, {id: 'layer/0/t0', frame: 109});
  const changed = structuredClone(doc);
  changed.definitions.card!.layers[0]!.tracks[0]!.keys[1]!.value = 0.5;
  c.replace(changed);
  assert.deepEqual(c.layout().visible, visible);
  assert.equal(c.layout().rows[0]!.expanded, true);
  assert.deepEqual(c.selection, {id: 'layer/0/t0', frame: 109});
  const retimed = structuredClone(changed);
  retimed.definitions.card!.layers[0]!.tracks[0]!.keys[1]!.frame++;
  c.replace(retimed);
  assert.equal(c.selection, null);
  assert.equal(c.cursor, 109);
  const replaced = structuredClone(retimed);
  replaced.definitions.card!.layers[0]!.name = 'Another layer';
  c.replace(replaced);
  assert.equal(c.layout().rows[0]!.expanded, false);
});

test('skimming follows the pointer without moving the parked timeline cursor', () => {
  const peeks: (number | null)[] = [];
  const seeks: SeekIntent[] = [];
  const timeline = new TimelineController(doc, intent => seeks.push(intent), frame => peeks.push(frame));
  timeline.setPlaybackFrame(20);
  const x = (frame: number) => timeline.frameToX(frame + 0.5);
  timeline.skimAt(x(32));
  timeline.skimAt(x(32));
  assert.equal(timeline.cursor, 20);
  assert.equal(timeline.skimFrame, 32);
  assert.deepEqual(seeks, []);
  timeline.zoomAt(0.5, x(32));
  timeline.pan(200);
  assert.ok(timeline.layout().visible[0] > 32);
  assert.equal(timeline.skimFrame, 32);
  assert.deepEqual(peeks, [32]);
  timeline.skimAt(x(50));
  assert.equal(timeline.skimFrame, 50);
  timeline.fit();
  assert.equal(timeline.skimFrame, 50);
  timeline.clearSkim();
  timeline.setSkimmingEnabled(false);
  timeline.skimAt(x(40));
  assert.deepEqual(peeks, [32, 50, null]);
  timeline.setSkimmingEnabled(true);
  timeline.skimAt(x(40));
  timeline.beginScrub(x(40));
  timeline.pointerUp();
  assert.equal(timeline.skimFrame, null);
  assert.equal(timeline.cursor, 40);
  assert.deepEqual(peeks, [32, 50, null, 40]);
  assert.deepEqual(seeks, [{ frame: 40, phase: 'scrub' }, { frame: 40, phase: 'exact' }]);
});

test('only visible rows can be hit; tracks come from the document', () => {
  const c = new TimelineController(
    {
      ...doc,
      placements: [...doc.placements, { ...doc.placements[0], track: 'V47', path: 'V47:card@0' }],
    },
    () => {},
  );
  c.resize(900, 82);
  assert.deepEqual(
    c.layout().rows.map((r) => r.label),
    ['V1'],
  );
  c.scrollRows(trackHeight);
  assert.deepEqual(
    c.layout().rows.map((r) => r.label),
    ['V47'],
  );
  assert.equal(c.hitTest(c.frameToX(51), 80)?.kind, 'time');
  const clip = c.hitTest(c.frameToX(49) + 0.2, 80);
  assert.equal(clip?.kind, 'clip');
  const hitEnd = c.hitTest(c.frameToX(50), 80);
  assert.equal(hitEnd?.kind, 'time');
});

test('media lanes and compact camera rows share scroll and hit-test geometry', () => {
  const placement = doc.placements[0];
  const controller = new TimelineController({
    ...doc,
    placements: [
      placement,
      { ...placement, track: 'A1', path: 'A1:card@0' },
      { ...placement, track: 'CAM', path: 'CAM:card@0' },
    ],
  }, () => {});
  controller.resize(900, bodyTop + trackHeight + rowHeight);
  assert.deepEqual(controller.layout().rows.map(row => [row.kind, row.top, row.height]), [
    ['lane', bodyTop, trackHeight],
    ['lane', bodyTop + trackHeight, rowHeight],
  ]);
  const lastVideoPixel = controller.hitTest(10, bodyTop + trackHeight - 1);
  assert.equal(lastVideoPixel?.kind === 'row' ? lastVideoPixel.row.label : null, 'V1');
  const firstCameraPixel = controller.hitTest(10, bodyTop + trackHeight);
  assert.equal(firstCameraPixel?.kind === 'row' ? firstCameraPixel.row.label : null, 'CAM');
  controller.scrollRows(999);
  assert.deepEqual(controller.layout().rows.map(row => [row.label, row.top, row.height]), [
    ['CAM', bodyTop, rowHeight],
    ['A1', bodyTop + rowHeight, trackHeight],
  ]);
});
