# Video editor milestones and tooling backlog

## Editor milestone

1. **Live project reload — complete.** A save is validated by the open session. The windowless client and viewer consume the same accepted revision and damage report. [Reload contract](../../../host/README.md#live-project-reload)
2. **Editor session.** A user selects an authored object in the picture, issues an edit and can reopen the result from disk. Rust owns interaction state, HUD geometry, history and writeback. Electron and MCP are development harnesses for the same editor contract. [Editor session RFC](../../../rfcs/0008-editor-session.md)

### Editor session tasks

- [x] **Pick and inspect native 2D layers headlessly.** Rust exposes selection and sampled geometry at a given document revision and project frame. The test checks a masked overlap against rendered pixels and distinguishes two uses of shared artwork. [Geometry and HUD](../../../rfcs/0008-editor-session.md#geometry-and-hud)
- [x] **Pick images and video by their display bounds.** Apply transforms and masks to media layers. A click on exposed implicit shot footage selects its timeline placement. [Geometry and HUD](../../../rfcs/0008-editor-session.md#geometry-and-hud)
- [x] **Prove the first persisted edit.** Root V2 visibility is a typed intent against a split master. The CLI exercises undo, redo, save and reopen, preserves comments, and rejects a conflicting external write. The [headless test](../../../engine/tests/editor_tracks.rs) checks root-track visibility, picture damage and persistence across reopen. [First headless proof](../../../rfcs/0008-editor-session.md#first-headless-proof)
- [x] **Wire the Electron harness to the core session.** Picture and timeline selection use the same native editor state. Rust rasterizes the preview outline. Signal Atlas exercises the track switch, undo, redo and save. [Try the editor](../../../host/README.md#try-the-editor)
- [ ] **Extend track controls beyond root V2.** Decide how V1 visibility interacts with timeline duration and nested audio. An authored A1 mute affects the delivered mix. A monitor-only mute belongs to the live session.
- [ ] **Build direct manipulation on those commands.** Follow selection and the first persisted toggle with moving a selected 2D layer in the preview. Convert from screen through the evaluated parent transform at the chosen frame, distinguish editing a base value from setting a key, preview the gesture and commit one undo step. Drive the same sequence of intents in a windowless test before binding handles in the app.
- [ ] **Identify the changed property or key inside an occurrence.** Use an authored identity that remains meaningful across a revision and can serve editor commands. [Review walkthrough](live-edit-demo.md)
- [ ] **Pick through transparent image pixels.** Add an alpha-aware image hit area over the accepted asset. Prove that transparent padding and holes can pass a click to the layer beneath.
- [ ] **Resolve picking through effects.** Keep an authored layer pickable for coverage-preserving effects such as RGB grade. Define how blur, shadows, erasure, and custom shaders contribute to the hit area or obscure targets underneath.
- [ ] **Select backdrop adjustments.** Expose the authored adjustment region for editing and define how its preview hit competes with the picture underneath.
- [ ] **Edit masks and spatial elements directly.** Give an attached mask an edit target under its owner. Add depth-aware picking for 3D meshes and planes and explicit editor targets for cameras and lights.

## Deferred tools

The current PoC work lives in the [gate list](poc-gates.md).

- [ ] **Report asset usage before pruning.** Scan the asset registry, master, and referenced detail files. For each asset, show where it is used, including uses found only in disabled placements or saved alternatives. List assets with no references as cleanup candidates. If a definition could not be inspected, flag the gap in the report so the creator can review it before pruning.
- [ ] **Print alternate text views of the timeline.** Let the creator or agent inspect the whole edit or one named sequence. Offer chronological order across tracks (X), grouping by track with its placements in time order (Y), and an outline grouped by nested sequence. Print frame ranges and placement names from the same authored master. Switching views leaves the file untouched.

## Live project reload and damage tracking

The deferred customer demo uses the shared Signal Atlas survey and a one-frame camera cut. Its recording and regression replay exercise the same edits. [Demo, timeline highlights and damage-tracking plan](live-edit-demo.md)

- [x] **Reload the open project at the parked frame.** The engine observes authored inputs and validates a candidate with its parked picture. The SDL harness consumes the signal in process. The headless client and Electron receive it over the native host protocol. A failed candidate keeps the accepted inspection and parked picture. [Run the headless or viewer trial](../../../host/README.md#live-project-reload)
- [x] **Show which timeline occurrences an external edit affected.** Report affected occurrence intervals and isolated thumbnail damage from Rust. Keep the unchanged gap, sibling tracks and warm tiles visible. [Signal Atlas regression and scope](live-edit-demo.md)
- [x] **Reuse clean preview frames and the prepared mix after a save.** The headless Signal Atlas regression checks two uses of the same grade, a reprise-only gain edit and a one-frame camera cut against cold opens. Run `just test-invalidation` for real reuse counters. Measure save-to-presentation latency separately when qualifying the editor experience. [Selective reload](live-edit-demo.md)
- [ ] **Watch resolved OpenUSD dependencies.** A change to a nested layer or texture currently needs manual reload. Observe the dependencies of each loaded stage and prove that editing a texture alone refreshes the affected placements. [Current behavior](live-edit-demo.md#what-the-engine-reports)
- [ ] **Keep linked video available to the accepted revision.** A decoder may reopen a video by path for an uncached frame. If the file was overwritten or deleted and reload was rejected, that seek can change or fail. Prove that a cold seek still returns the previous pixels after a rejected replacement. [Current behavior](../../../host/README.md#live-project-reload)

## Motion typography

- [ ] **Keep shaped text addressable for animation.** N4 currently keeps the editable string and paints the wrapped paragraph to one RGBA image. Retain positioned glyph runs, font identities and source-text clusters when an edit needs access to those results. Enlarge and rotate an outlined glyph, then change a word containing a ligature or combining mark. Compare edge quality, shaping, scrub response and preview/export agreement before changing rasterizers. [Current text renderer](../../../engine/src/render/text.rs) · [Kinetic headline exercise](production-recipes.md#a-kinetic-headline) · [egui text layout](https://github.com/emilk/egui/blob/master/crates/epaint/src/text/text_layout.rs)
