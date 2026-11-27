# Engine and production qualification

Use these gates to decide whether a change can be promoted from the spike. The [regression guide](../../../../docs/tests.md) identifies checks that a contributor can run now. A target below still requires a recorded result on the intended platform.

## P0 · Fix the target and the pass criteria

Record platform, GPU, build profile, project dimensions, frame rate, codec, input duration and whether caches are warm. Declare each acceptance threshold before measuring. Keep full-quality paused inspection, playback under load, and final encoded delivery as separate observations. Use the original CC0 projects for shared evidence. [Performance guide](../../../../docs/performance.md) · [Production recipes](production-recipes.md)

| Gate | Check |
| --- | --- |
| Native evaluation | Validate and inspect the master. Step across a cut, a nested animation and the second placement of a shared sequence. Compare explicit source times. |
| Media clock | Compare a fresh seek with forward playback around a keyframe, a variable-frame-rate sample and an audio cut. Verify exported picture timing and the decoded audio span. |
| Graphics | Replace a title, change its wrap width, animate a path and mask a moving image. Inspect a fractional edge and an overlapping foreground object. |
| Responsiveness | Scrub in both directions while thumbnails populate. Record input time, actual presentation, late frames and work discarded when the desired target changes. |
| Export | Compare native inspection and pre-encode output at the same time, geometry and quality. Check the decoded movie separately for encoding errors, sync and length. |
| Revision and history | Open one project in a persistent session. Change a shared definition, then only whitespace. Verify reported damage, retained work, rejected edits and undo/save/reopen. |
| Distribution | Follow the contributor setup from a fresh clone. Run an SDK-independent project headlessly and open it in the harness. Test optional spatial support separately with the documented SDK. |

## P4 · Paint, animate and reuse real 2D graphics

Change an editable title while its neighboring objects continue to use the same media. Scrub across the edit, then seek directly to a middle frame. Inspect glyph edges, mask boundaries, parent transforms, stroke widths and neighboring samples read by a blur. Compare pixels or a stated numerical tolerance when paths differ. [Graphics inventory](graphics-capabilities.md) · [Rendering pipeline](rendering-pipeline.md)

## Mixed 2D and 3D

Use the recorder in Field Unit to check shared depth, independent screen artwork and camera cuts. A stage opening successfully proves file access. Check the actual meshes, bound materials, transforms and occlusion for each schema or importer feature being claimed. [Field Unit](../../../examples/field-unit/README.md) · [Spatial coverage tracker](../../../spatial/openusd/PROFILE.md)
