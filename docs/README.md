# N4 specifications and design notes

Use the current contracts when implementing a reader, renderer or host. The research notes and early RFCs also describe ideas outside the alpha. A proposal becomes supported when the engine accepts it and a contributor can reproduce the behavior.

| Current contract | What it owns | Executable evidence |
| --- | --- | --- |
| [Authoring project](project-format.md) and [visual XML](../spikes/docs/research/video-editor/sequence-xml.md) | Placement identity and timing in the master, local artwork and animation in a detail, asset resolution | [Signal Atlas master](../spikes/examples/signal-atlas/project.proj), [master tests](../spikes/engine/tests/master.rs), [SDK edits](../spikes/sdk/python/tests/test_xml_authoring.py) |
| [Editor session](../spikes/rfcs/0008-editor-session.md) | Revision-scoped selection, geometry and picking, HUD, history, conflict detection and save | [Command-pipe tests](../spikes/engine/tests/editor_cli.rs), [geometry tests](../spikes/engine/tests/editor_geometry.rs) |
| [Host and live refresh](../spikes/host/README.md) | A persistent native session, frame delivery and return, input watching, independent output and thumbnail damage | [Wire client](../spikes/host/kit/src/client), [selective invalidation](../spikes/host/test/selective-invalidation.mjs) |
| [Spatial profile](../spikes/spatial/README.md) | Shared 2D/3D depth, imported geometry and materials, scene integration and current coverage | [OpenUSD import profile](../spikes/spatial/openusd/PROFILE.md), [headless GPU host](../spikes/spatial/wgpu/examples/headless.rs) |
| [Path-traced Final](../spikes/rfcs/0009-path-traced-final.md) | Draft and Final for a spatial scene, the identity of a finished picture, range rendering and export through an optional Cycles bridge | [Engine tests](../spikes/engine/tests/path_trace.rs), [qualification script](../spikes/spatial/cycles/tools/qualify.py) |

## Decisions and open questions

- The [production recipes](../spikes/docs/research/video-editor/production-recipes.md) describe edits to try and the frames to inspect when qualifying a capability.
- The [architecture overview](../spikes/docs/research/video-editor/architecture-overview.md) and [rendering pipeline](../spikes/docs/research/video-editor/rendering-pipeline.md) describe the intended boundaries. The [V1 proposal](../spikes/rfcs/0005-video-editor-v1.md) records the original scope.
- The [creative vocabulary](../spikes/docs/research/video-editor/ledger.md) and [graphics inventory](../spikes/docs/research/video-editor/graphics-capabilities.md) connect production techniques to primitives and open questions. The [design precedents](../spikes/docs/research/video-editor/design-precedents.md) explain why an N4 concept exists.
- Use the [regression guide](tests.md) and [performance guide](performance.md) to choose a test and its measurement boundary. Remaining editor work is recorded beside its implementation and in the [tooling backlog](../spikes/docs/research/video-editor/toolings.md).
