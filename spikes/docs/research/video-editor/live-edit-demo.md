# Show what changed after a save

A creator can park on a frame, save an external edit and see the affected portions of each clip briefly outlined and tinted. The headless session and Electron consume the same accepted-revision contract. [Try the live reload](../../../host/README.md#live-project-reload)

## The demo to record

Copy [Signal Atlas](../../../examples/signal-atlas/README.md) into a working directory. Open it in the viewer and park at frame 42. In `sequences/survey.xml`, add `<rgb-grade gain="1 1 0.65"/>` to the image named **Synthetic bathymetry** and save. The preview changes under the parked playhead. The viewer briefly outlines the survey at both ends of the timeline. Its middle shot stays clear.

In the master, change the reprise's touching camera boundaries from 12 to 13. Only film frame 140 changes view. Save again with only a whitespace edit. The viewer should preserve existing 2D thumbnails and show no change cue. [Runnable regression](../../../host/test/selective-invalidation.mjs)

## What the engine reports

Blender’s Video Sequence Editor follows a changed strip to dependent effects and invalidates the final-image cache over the resulting film interval. It also has separate calls for source-image and thumbnail caches. N4 borrows the distinction between an edit’s effect on the timeline and the particular cached outputs that depend on it. Shared definitions in N4 can affect disjoint placements, so its revision contract preserves the gap between uses. [Blender VSE strip relations](https://github.com/blender/blender/blob/main/source/blender/sequencer/intern/strip_relations.cc)

An accepted reopen reports the old and new revision and the potential damage to each output. The evaluator compares validated projects and the identities of the resources they read. Changing a shared picture can produce multiple disjoint film intervals. An audio range includes each project frame containing an affected sample. Uncertain dependencies expand the scope conservatively. The watcher signals a possible change earlier in this process. Root track visibility edits already use the same damage evaluator. [Session contract](../../../rfcs/0008-editor-session.md)

The shared survey edit reports picture damage on `[0,64)` and `[128,192)`. At frame 64 the native frame cache retains the warm picture. A seek to 140 fetches the changed view. Muting the reprise instead changes audio and waveform peaks. The regression compares each accepted picture against an independent fresh open.

A change to film duration or frame rate invalidates both outputs. A changed shot count or overlay count conservatively invalidates the picture. Each OpenUSD placement also invalidates its enclosing shot on reopen because transitive USD inputs can live outside the observed files. Editing a transitive texture alone still requires a manual reload. Until the thumbnail worker shares a frozen import or can prove identical resolved mesh and texture inputs, a reopened 3D shot can regenerate thumbnails even when its XML change is only whitespace. That uncertainty alone creates no clip highlight.

The first pass identifies occurrences from the two accepted projects. The producer reports each current path together with disjoint project-frame intervals. The viewer uses these intervals for the transient cue. A formatting-only Signal Atlas save emits two empty lists. A rejected save retains the last accepted picture. Neither triggers a cue.

Thumbnail reuse has its own evidence because a clip tile has a different input scope from the final frame. Grade the V1 survey and its V2 station overlay stays reusable, even though both occupy the same output frames. Edit a V2 *inside* a V1 and the containing V1 thumbnail can need a refresh as well. The consumer retains successful bitmaps for matched paths and clean frame samples. It can repaint the canvas using those same bitmaps. If the evaluator cannot establish the correspondence across revisions, the viewer can mark the film ranges on its ruler and refresh uncertain tiles without assigning a change to a specific clip.

| Question | Evidence |
|---|---|
| What did the author change? | Compare the accepted definitions and placements. Report the property, key or cut and its old/new positions where applicable. A row rename can change the inspector alone. A formatting-only save produces no edit highlight. |
| Where might the edit affect output? | Report separate half-open ranges per occurrence and output, including picture or audio. Preserve gaps. Mark a conservative range as an estimate when a dependency is opaque. |
| What work ran, and did the result differ? | Trace cache reuse, invalidated entries and actual renders separately. The harness can verify pixel and sample differences offline. A fully obscured layer may change without changing the final picture. Scrubbing can render an uncached frame without an authoring change. |

## Current proof and remaining work

1. **Correct reload.** The headless session and viewer share the full-rebuild baseline. A revision switch carries the inspection snapshot and parked picture together. Rejected candidates retain the accepted revision and report a diagnostic. Use this baseline as the oracle when introducing partial invalidation. [Host contract](../../../rfcs/0007-electron-host-kit.md)
2. **Exercise selective reuse.** Run `just test-invalidation`. Signal Atlas changes a shared grade, mutes the reprise and rolls a camera cut. The test warms pictures, reopens the edited project and compares work counters, waveform peaks and pixel hashes with independent fresh opens. A malformed save keeps the prior revision available. A formatting edit produces empty damage for this film. [Headless trial](../../../host/test/selective-invalidation.mjs)
3. **Show the changed placements.** The timeline consumes the occurrence intervals and thumbnail damage from the accepted revision. Node tests cover the ranges shown for each placement and cache reuse. Identifying the exact layer or property within an affected occurrence remains an editor milestone. [Timeline model](../../../host/kit/test/timeline.test.ts) · [Thumbnail cache](../../../host/kit/test/thumbnail-cache.test.ts)

## Headless regression fixtures

Replay saves in a temporary project copy through the native session. Check revision, occurrence, predicted range, native work and delivered pixels against an independent expectation.

| Fixture and edit | Independent check |
|---|---|
| Shared survey grade | Check both occurrences, a warm frame in the gap, each isolated thumbnail and unchanged audio. |
| Reprise camera cut, 12 → 13 | Compare frames 139, 140 and 141 with fresh renders. Only 140 selects a different camera. |
| Save during a scrub, invalid save and repair, formatting-only save | Check for late thumbnails or pictures from an older revision. A failed save reports the diagnostic and keeps the last accepted movie and selection. Repair updates the same window. A formatting change alone generates no film ranges. |

Compare the reloaded revision with a fresh open at matching quality. Check the changed frame, both sides of cuts and an unaffected warm control. Measure end-to-end presentation latency separately when investigating a presenter. [Engine cache measurements](../../../../docs/performance.md#a-small-edit-that-redraws-a-scene)

## How film damage differs from browser damage

Chromium distinguishes paint invalidation, raster invalidation and compositor damage. A layer move can expose old screen space and cover new space while its content texture remains reusable. DevTools paint flashing exposes actual repaints. Premiere’s render bars show preview readiness. N4's timeline cue identifies the portions of the film an accepted edit can affect. [Chromium compositor](https://chromium.googlesource.com/chromium/src/+/main/docs/how_cc_works.md) · [Chrome paint flashing](https://developer.chrome.com/docs/devtools/rendering/performance/) · [Premiere render bars](https://helpx.adobe.com/ie/premiere/desktop/render-and-export/render-sequences-for-playback/render-a-section-of-a-sequence.html)

| Film-specific concern | Consequence for damage |
|---|---|
| Project time, placement time and source time | One shared definition can affect disjoint film intervals, including past and future frames. Intersect with each trim and map through nesting. Retimes, holds and reversals can map one source frame to many output frames. Scrubbing asks for them in arbitrary order. [OTIO time ranges](https://opentimelineio.readthedocs.io/en/latest/tutorials/time-ranges.html) |
| Keys, cuts and transitions | Moving a key may affect interpolated values on either side. A hold, a cubic segment and a cut have different footprints. A ripple edit can shift the whole downstream schedule while leaving source decodes reusable. A dissolve depends on both clips across the overlap. |
| Effect inputs in space and time | A backdrop samples lower layers. A blur reads surrounding pixels. A shutter sample or echo may depend on times outside the output frame. Work backwards from the output's declared inputs, then invalidate every output whose inputs can intersect the edit. For an unknown custom effect, use a conservative enclosing scope. OpenFX has explicit queries for input regions and required frames. [OpenFX image processing](https://openfx.readthedocs.io/en/latest/Reference/ofxProcessingArch.html) · [OpenFX effect actions](https://openfx.readthedocs.io/en/latest/Reference/ofxImageEffectActions.html) |
| Different products of one edit | Sound uses sample time. Picture uses project frames. A gain change may affect the mix and its waveform while composed pictures stay valid. A camera edit can affect composed thumbnails while keeping source thumbnails and decoded footage reusable. Proxy previews and full-quality export have separate cache identities. |

The first demo needs authored ownership, disjoint film intervals and a trustworthy full-reload baseline. A per-pixel damage map, a persistent render cache and a general-purpose dependency graph can each earn their place through a later measurement.
