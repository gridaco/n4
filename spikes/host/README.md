# Embedding the N4 viewer

The host kit opens an N4 project in a parent-owned Electron window. Rust delivers the composed picture, timeline inspection, waveform and thumbnails. For a split master, the harness also uses the Rust editor session for picking, selection, HUD pixels and edits. TypeScript presents the picture and timeline and forwards input.

## Run the harness

The native renderer currently targets Apple Silicon macOS. Install its [build prerequisites](../engine/README.md#build-and-run) and the standard Node.js 24 runtime. From the repository root, install the package manager pinned by the workspace and open the example.

```sh
npm install --global "$(node -p 'require("./spikes/host/package.json").packageManager')"
just viewer spikes/examples/signal-atlas --frame=42
```

Drag the ruler or the playhead handle to scrub. Press Space to play or pause. Double-click a placement to enter it. Use the sidebar buttons to expand a layer, then click a key to see its picture. Escape returns to the enclosing timeline. The sidebar shows the requested project time and the badge over the preview shows the time of the submitted picture. The ruler starts at local zero in each nested timeline. [Native setup](../engine/README.md)

Each video clip has a filmstrip anchored to its start. Panning reveals or crops tiles at the viewport edges. Zoom changes the sampling interval in steps. When the interval changes, a cached picture covering the same part of the clip can stay visible until the exact sample arrives. The viewer requests one visible sample at a time and pauses new requests during playback, dragging, and active preview seeks. Thumbnails continue loading once a hovered frame is ready.

Move the pointer across the paused timeline to skim the picture. A blue line marks the skimmed frame. Panning and zooming preserve that frame until the pointer moves again. Modifier-wheel zoom uses the exact timeline position under the pointer. Press Space while skimming to start playback there. Move outside the ruler or lanes to restore the parked picture and play from that position.

Drag the divider above the timeline to resize it. With the divider focused, use the Up and Down arrows to adjust the height or Home and End to reach either limit.

Click **Export** in the viewer to open the **Export Video** window. It shows the project’s resolution, frame rate and duration. Choose a new filename there to save the accepted revision as an H.264/AAC MP4. Playback pauses while the native engine renders. The export window shows progress, lets you cancel and reports the saved file when the encoder finishes.

## Try the editor

Copy the public motion example before editing:

```sh
trial=$(mktemp -d)
cp -R spikes/examples/signal-atlas "$trial/film"
echo "$trial/film"
just viewer "$trial/film" --frame=42
```

At frame 42, hover or click the orange status light at the upper left. The native HUD shows its outline and the timeline selects the Video 2 occurrence. Drag across the art to select multiple layers. Use the **On** button next to Video 2, then **Undo**, **Redo** and **Save**. Reopening the project restores the saved track state. [Editor contract](../rfcs/0008-editor-session.md) · [Headless transport check](test/native-editor.mjs)

## Live project reload

In the scratch copy, open `sequences/survey.xml`, change the background from `#eeebde` to `#dedccd` and save. The parked picture updates. Both placements of the shared survey light up and the middle shot stays clear. Saving only whitespace leaves the thumbnails untouched. A malformed save produces a diagnostic and the last accepted revision stays available until you repair the file.

To see the same transitions as JSON, run `just watch-project <scratch-directory> 42` in another terminal. The first `ready` record is followed by an event each time the engine accepts or rejects an edit. **Reload** in Electron requests another candidate explicitly.

The Rust engine owns file observation. A background worker samples the loader’s inputs every 250 ms and waits one quiet interval after a change. It covers the master, sequence details, directly referenced media, fonts, SVGs and shaders, and paths consulted during a rejected load. Imported USD stages are watched at the root asset path. The SDL player and inspector consume the hint in process. For Node and Electron, the native sidecar sends an unsolicited `inputsChanged` record on the control pipe. The client can request a reopen at its parked frame. An active export defers the notice. [Engine watcher](../engine/src/project_watch.rs) · [Native service](../engine/src/bin/n4-host.rs)

Changes only to a nested USD dependency, such as a texture, need a manual reload. Video decoders can reopen linked files by path for uncached frames. Replacing a video at the same path can affect a later seek even if the reload was rejected. The accepted revision keeps its parked picture and prepared artwork. [Video decode](../engine/src/media/native.rs)

To compare the native window with Electron, run `just play <printed-project-directory>` or `just inspect <printed-project-directory>` and park on the same edit. Each open project has its own engine session.

For a windowless consumer, `session.onProjectChange(listener)` signals a possible disk change. A successful `session.reopen(frame)` returns the new inspection, the validated parked picture and a revision transition. The viewer delivers that transition through `onRefresh(document, change)`. Protocol v4 reports potential damage to picture and audio independently. Each is `{kind:"none"}`, `{kind:"all"}` or `{kind:"ranges",ranges:[[start,end],...]}` in project frames. The intervals are sorted, disjoint and half-open. A consumer can use this report for any accepted edit regardless of the host. The watcher hint alone does not advance a revision.

The same transition carries `change.timeline`. In a `known` report, `occurrences` identifies the changed placements and `thumbnails` identifies the isolated clip samples that can have different pixels. Each entry contains a current placement path and half-open project-frame ranges. The viewer briefly outlines and tints each affected visible range. After about a second the clip returns to its ordinary appearance. Click and double-click keep their usual timeline actions throughout. A formatting-only save gives no cue. For an `unknown` report, the viewer indicates the affected time on the ruler when damage ranges are available.

A V1 thumbnail includes its nested artwork. A sibling V2 thumbnail samples that sibling alone. Retain an existing tile when its placement still exists and its sampled frame stays clean. An `unknown` report requests conservative refresh. Every inspection includes a native `sessionId`. A process restart begins a new cache lifetime even when the revision numbers happen to match.

Run `just test-invalidation` for the headless proof. It edits a private copy of Signal Atlas and checks both survey placements against a fresh session. The test also covers independent audio gain, a one-frame camera cut and a formatting-only save. [Regression](test/selective-invalidation.mjs)

The first open needs a valid project. After that, a rejected save keeps the accepted inspection and parked picture. Seeking an uncached frame of deleted or overwritten footage can still require the original media bytes.

## Develop the host

The pnpm workspace in this directory contains the embeddable `kit` and its reference `harness`. TypeScript 7 checks the sources. esbuild bundles Electron main, the isolated preload and the renderer separately. Tailwind CSS compiles the harness stylesheet locally, and the vanilla Lucide package supplies its controls. A parent app can mount the kit with its own chrome and styles.

```sh
cd spikes/host
pnpm install --frozen-lockfile
pnpm run build
pnpm test
```

Use `pnpm run typecheck` for a compiler-only pass. For UI work, leave `pnpm run watch` running and launch `pnpm start ../examples/signal-atlas` in another terminal from this directory. The watcher rebuilds the bundles, stylesheet and HTML when the kit or harness changes. Reload the window after a renderer change and relaunch Electron after changing main or preload.

## Use the kit in a parent

The parent grants one project and one local renderer URL per binding. Supply the path to the packaged native executable. For export, the binding exposes `startExport(revision)`, `exportStatus()` and `cancelExport()` in main. The parent provides a `chooseExportPath` callback that grants a destination from its save dialog. The harness uses this API from a separate window with its own preload. [Export window](harness/src/main/export-window.ts)

```ts
// Main process
import { bindViewer } from '@n4/host-kit/electron-main';

const disposeBinding = bindViewer(window.webContents, { executable, project, rendererURL });
```

```ts
// Sandboxed preload
import { exposeViewer } from '@n4/host-kit/electron-preload';

exposeViewer('film');
```

```ts
// Renderer
import { mountViewer } from '@n4/host-kit/viewer';

const viewer = await mountViewer(window.film, {
  preview: previewCanvas,
  timeline: timelineContainer,
  frame: 42,
  onError: showDiagnostic,
});
viewer.seek(76);
```

Supply a preview canvas and a sized HTML container for the timeline. Import `@n4/host-kit/timeline.css` in the parent’s CSS build or link the resolved file as a stylesheet. The kit mounts its sidebar, playhead and content canvas inside the container. The parent owns shortcuts outside the timeline and decides where to show diagnostics. Use `viewer.play()`, `viewer.pause()` and `await viewer.reopen()` for transport controls. Call `viewer.dispose()` when unmounting the view. Call `disposeBinding()` in main when revoking a grant or replacing the bound project. Destroying the window also releases its service. [Minimal independent consumer test](test/parent-consumer.mjs) · [Harness renderer](harness/src/renderer/index.ts)

Use `@n4/host-kit/client` from Node for a windowless session. `NativeSession.open(executable, project)` accepts a project directory or entry file. `session.document.snapshot` describes placements and shared definitions. For concurrent requests use increasing transport generations. Intermediate seeks can be superseded. Await the exact release target and call `await session.close()` when the document closes. The lower-level playback API requires the consumer to acknowledge submitted pictures. The supplied presenter does this automatically. [Client](kit/src/client/index.ts) · [Transport interface](kit/src/client/types.ts)

## What to run when changing the host

| Change | Check |
|---|---|
| Interaction or transport | `just host-test` runs controller tests in Node, canonical project inspection and byte-for-byte frame comparison with the native CLI. With the optional SDK, `N4_TEST_OPENUSD=1 just host-test` also checks Field Unit. |
| Live project reload | `just host-test` verifies native save notices, rejected edits, repair and selective thumbnail invalidation through the headless client. |
| Embedding | `node spikes/host/test/parent-consumer.mjs` mounts, disposes and remounts from a fresh parent using the kit exports. |
| Timeline input | `just host-test` covers selection, hit testing and navigation. If changing Chromium focus handling, `pnpm exec electron test/electron-inspector.cjs` from `spikes/host` checks that Space still controls playback after grabbing the playhead. |
| Electron export integration | When changing the native save dialog or export window lifecycle, run `pnpm exec electron test/electron-export.cjs` from `spikes/host`. |
| Preview delivery | Build `n4-player` with the native viewer prerequisites. Run `python3 -B spikes/host/test/compare-scrub.py /tmp/n4-scrub-review` with a **new** output directory. The runner opens foreground test windows serially and saves each raw trace. |
| Runtime packaging | In this directory run `pnpm run build`, then `pnpm run package "/tmp/N4 Viewer.app"` with a new output path. From the repo root run `node spikes/host/harness/test/packaged.mjs "/tmp/N4 Viewer.app" spikes/examples/signal-atlas 42`. This tests a local ad-hoc signed macOS bundle with its media tools and native libraries. |

The current presenter copies bounded RGBA8 frames through a binary pipe and into a canvas. Use the native and Node tests for engine and timeline changes. Use an Electron check when the behavior depends on the host, such as preload isolation, keyboard focus, presentation pacing or packaged runtime discovery. [Host RFC](../rfcs/0007-electron-host-kit.md) · [Performance guide](../../docs/performance.md)
