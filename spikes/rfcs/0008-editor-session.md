# RFC 0008 — Native editor session

One Rust session owns the working project, playhead, scope, selection, HUD and history. A client submits input or a typed edit and presents the answer. The Electron harness uses this boundary. A future MCP wrapper can translate tools into CLI commands. MCP schemas and transport belong to that harness.

## Addressing and state

An open session supplies a session ID, document revision and HUD version. An accepted edit advances the document revision. Seeking or selecting can advance the HUD version independently. A revision is meaningful within its session. Commands that depend on document content name the expected revision. A client picking from a displayed picture supplies the revision and frame of that picture. A request for a stale document receives a structured error.

A layer reference contains the concrete placement path and an array of zero-based authored child indexes. Two uses of the same definition have distinct occurrence paths. Both resolve to the same definition file. The selected track and placement point to the master. Layer references are revision-scoped. Retain selection on reload only when the target still identifies the same authored layer unambiguously. A structural edit can clear selection.

The session returns the active scope, playhead, selection, track states, HUD geometry, dirty and conflict flags, and undo and redo availability. Rust resolves semantic selection and geometry. A presenter supplies pointer coordinates and measured viewport information as needed and paints the returned HUD. The first CLI accepts composition coordinates directly, in pixels from the top left of the output.

## Control API

Build the native tools with `just host-native`. Open a persistent control pipe with `spikes/engine/target/debug/n4 editor PROJECT`. Each JSON line has an integer `id` and an `op`. Replies retain the request ID and carry `result` or an `error` with `code` and `message`. The pipe owns one session until EOF. Clients that share one owner see the same state. A future attach adapter must route to the running owner to inspect its user selection.

```sh
printf '%s\n' '{"id":1,"op":"getState"}' '{"id":2,"op":"inspect"}' \
  | spikes/engine/target/debug/n4 editor spikes/examples/signal-atlas
```

| Operation | Arguments and result |
| --- | --- |
| `getState` | Return the session state, including `sessionId`, `revision`, `hudVersion`, `frame`, `scope`, `selection`, `selectionSource`, `hud` and the tracks in scope. |
| `inspect` | Return the working revision's placements and shared definitions. |
| `seek` | Set the playhead to a project `frame`. Return state. |
| `enterScope` | Supply `revision` and `scope`. Use `root` or a nested timeline occurrence. Return state. |
| `hitTest` | Supply `revision`, `frame` and `point` as `[x,y]`. Return hits ordered front to back and report unsupported coverage when it can obstruct the answer. A clear point over exposed implicit shot footage returns its `fallbackPlacement`. |
| `selectAt` | Use the hit test arguments to select the frontmost known layer or the fallback shot placement. A clear miss clears selection. Uncertain coverage ahead of every hit returns `indeterminateHit`. |
| `hoverAt`, `clearHover` | Hover at a revision, frame and point, or clear hover at a revision. Return state with an optional `hover` layer reference. |
| `beginMarquee`, `updateMarquee`, `finishMarquee`, `cancelMarquee` | Begin at a revision, frame and point, with an optional drag `threshold` in picture pixels. Update and finish send the revision and point. Cancel sends the revision. Return state with the active `marquee` bounds and ordered `selections`. |
| `layerGeometry` | Supply `revision`, `frame` and `layer` as `{occurrence,layers}`. Return the sampled layer pose and visibility. |
| `select`, `clearSelection` | Select a checked `target` at the given `revision`, or clear it. A layer target has `kind: "layer"`, `occurrence` and `layers`. A placement has `kind: "placement"` and `occurrence`. A track has `kind: "track"`, `scope` and `track`. |
| `setTrackEnabled` | Supply `revision`, `scope`, `track` and explicit `enabled`. Return state and damage when the edit changes the document. |
| `undo`, `redo`, `save`, `reload` | Supply `revision`. `reload` accepts `discardChanges: true` to discard a dirty draft explicitly. |
| `render` | Sample the working document at `revision` and `frame`. Optional `points` request a bounded list of RGBA pixels for headless checks. |

The CLI control pipe can announce a `stateChanged` event after an external save or conflict. The Electron harness opens `n4-host --editor PROJECT` and sends the same commands through its existing control pipe. An accepted edit sends the new inspection and parked picture together. A `hud` request supplies the displayed revision and frame and returns a versioned transparent RGBA tile with an origin in picture pixels. Rust draws that tile. The presenter blits it over the matching picture, separately from the film that it exports. [Run the editor harness](../host/README.md#try-the-editor)

## Geometry and HUD

Sample the same placement clock, content, motion, transforms and camera as the painter. `layerGeometry` returns the nominal layer pose, authored bounds, and the sampled clips from its ancestors through the layer itself. Each clip supplies its coordinate map, its mask's raster extent when present, and the active wipe edge when present. An outline can use known authored bounds when painted coverage is uncertain.

Point queries traverse the active scopes front to back. Rectangles, paths and imported SVGs use alpha coverage. An SVG is one selectable asset. Native text uses the occupied part of each visible line, including spaces within it. The region follows sampled reveal and glyph displacement. An isolated group's mask can rule out its subtree. Inside the mask, a point query reaches the frontmost eligible child. The group remains addressable from the hierarchy. Unsupported artwork or compositing can return uncertain coverage. A known clip that excludes the point permits traversal to continue below it.

Image and video hit areas use rectangular display bounds with the authored transforms, wipes and masks. Image dimensions come from the accepted asset. Independent video uses its declared size and the owning paint scope's active source interval. An explicit `<footage>` layer uses the project size. Clicking exposed implicit shot footage selects the shot placement after a clear point miss. Hover and marquee target authored layers.

The accepted revision owns image dimensions, prepared SVG rasters, shaped labels, pinned font faces and resolved text anchor offsets. The editor reads these on demand and stores sampled native coverage in a bounded editor cache. Render workers use the same accepted artwork with private GPU caches. Playback and export do not execute editor queries. A candidate replacement is prepared and validated before adoption. [SVG regression](../engine/tests/editor_svg.rs) · [Media, text and isolated-group regressions](../engine/tests/editor_geometry.rs)

The selected occurrence and its geometry produce the HUD at the current playhead. This also gives the live refresh viewer a way to draw a future layer cue in composition space. The existing occurrence damage report locates affected time ranges. Exact external layer attribution supplies a more specific target once the authoring diff can identify it.

Hover samples the frontmost supported hit and draws a lighter outline. The marquee follows the pointer while dragging. Release replaces selection with the eligible layers whose transformed regions intersect it. It clips the region through the ancestor mask bounds and wipes in their own coordinate spaces. Mask alpha, including transparent holes inside those bounds, is sampled by point queries. Front-to-back order determines the primary selection. A release below the drag threshold performs a click. Cancel preserves the selection. Hover and marquee updates affect HUD state and Rust-rasterized pixels.

## History and writeback

The master owns the first track edit. `track V2 off` after the header hides the overlays on root V2. Absence means enabled. `track V2 on` enables it explicitly. The lane stays available for timeline selection and isolated thumbnails. The shared frame planner omits its contribution to picture, picking and export. Nested V2 lanes keep their own scope. V1 duration and audio remain as authored.

Apply, undo and redo prepare a candidate and validate the parked frame before installing a revision. Keep reversible master text in the session. A semantic no-op leaves the revision and history alone. Undo and redo work across saves within that session. The saved track value is read back by a fresh process.

Save checks the revision and the accepted disk inputs, then atomically replaces the master through a temporary file in the same directory. The edit changes one directive and preserves surrounding text. Cooperating writers take an advisory lock. Ordinary text editors can still race the final rename. The watcher acknowledges the editor's own save as the new disk baseline.

A dirty session preserves its draft on an external edit and reports a conflict. Accepting a clean external change starts a new history baseline. An explicit discard can reload from disk. The existing native reload transaction supplies validation and selective invalidation for both external and session edits.

## First headless proof

Run `just editor-test`.

1. Compare a masked, animated pair of native layers against rendered witness pixels. Query geometry twice and use the same layer address for hit testing and selection.
2. Toggle root V2 in Signal Atlas. Check the badge at frames 42 and 140, the unaffected middle shot at 86, the two disjoint picture ranges and clean audio.
3. Undo, redo and save. Check the exact master diff and fresh-open the result. Keep a dirty session alive while a second writer changes the master and verify that a conflicting save preserves both edits.
4. Pick the station symbol and wrapped survey title in their composed picture. Check click-through, marquee and geometry across a transformed isolated mask and a repeated placement. Edit a wrapped label, reload, verify the attached graphic moves, and reject a query for the old revision.
5. Pick the relief image through the plate mask. Check that a transformed video only picks during its source interval and that exposed implicit footage selects its shot placement.
