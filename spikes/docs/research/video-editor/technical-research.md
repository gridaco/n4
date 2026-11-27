# Video editor technical research

N4 is a Rust video engine incubated in [Grida’s nothing project](https://github.com/gridaco/nothing). The alpha accepts a timeline master, visual XML and an asset registry. A native session serves the CLI, SDL2 viewer and Electron harness. The same project should be useful to a creator reading the text, an editor inspecting a particular frame and a program requesting an export. [Implemented contracts](../../../../docs/README.md)

## From a project to a frame

The master determines which placements are active at a project frame. Evaluation maps that frame into the local clocks of each placement and layer. A player can skip a late presentation. An export must request every output frame. Decoders select media by presentation time and the mixer works in samples. Timing tests need cuts, held frames and repeated placements of the same source. [Timeline precedents](design-precedents.md) · [Pipeline](rendering-pipeline.md)

Rust uses FFmpeg for media, Skia to rasterize supported artwork and text, and wgpu for GPU composition. A clipped group, backdrop filter and depth-aware scene each need a different set of inputs to produce a picture. A single flattened image is appropriate at some boundaries, including imported SVG artwork, while masks and object mattes preserve information needed by later effects. [Graphics inventory](graphics-capabilities.md) · [Spatial contracts](../../../spatial/README.md)

## Bevy as a rendering architecture reference

Study Bevy's separation of simulation state from prepared render resources and draw work. A video editor also needs to evaluate a requested instant with no earlier frames in memory, interrupt a seek and evaluate the same frame in a fresh process. Put those obligations on N4's evaluator and media providers. Borrow architectural patterns where they preserve that behavior. [Bevy render module](https://docs.rs/bevy_render/) · [Performance guide](../../../../docs/performance.md)

## OpenUSD as a 3D scene reference

OpenUSD provides a composed scene graph, geometry, transforms, cameras, animation and material bindings. A USD stage can describe the mechanism in a product shot. The N4 master schedules the shot and its sound, and the sequence places native graphics and imported objects into the same composition. File-format support, schema coverage and rendering quality are separate qualification questions. [Two authoring paths](usd-and-xml.md) · [Tested import profile](../../../spatial/openusd/PROFILE.md)

## Graphics, layout and external renderers

Use fixed stage dimensions, local transforms and explicit paint order. Text has its own shaping and wrap width. A shared surface contract lets a later text provider change rasterization while image effects continue to consume premultiplied color and alpha. Distinguish a glyph's outline or a curve's tangent from properties recoverable from finished pixels. A future provider that supplies only a rendered movie can participate in cutting and blending. It needs additional inputs or outputs for selection inside its scene, relighting or shared depth. [Path and mask boundary](native-path-spike.md) · [Spatial rendering proposal](../../../rfcs/0006-spatial-rendering.md)

## Editing contracts

Study OTIO for editorial time and nesting, MLT for media producers and filters, OpenFX for what a host must declare to a filter, and Blender's sequencer for output-specific invalidation. These references describe different boundaries. Compatibility requires an interchange or execution test against the actual external consumer. An N4 session owns accepted revisions, selection, history and damage. A host receives picture frames and typed notifications. [Design precedents](design-precedents.md) · [Editor session](../../../rfcs/0008-editor-session.md) · [Host protocol](../../../host/README.md)
