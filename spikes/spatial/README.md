# Spatial rendering and OpenUSD coverage

N4 lets a project direct an imported rigid object and paint native artwork onto its surfaces. The [Field Unit project](../examples/field-unit/README.md) demonstrates this in a six-second product spot. The [import profile](openusd/PROFILE.md) describes the accepted USD inputs and [RFC 0006](../rfcs/0006-spatial-rendering.md) records the architectural decisions.

## Development boundaries

[nothing-spatial](scene/src/lib.rs) owns the CPU scene data. [nothing-openusd](openusd/src/lib.rs) translates a composed USD stage into that data. [nothing-spatial-wgpu](wgpu/src/lib.rs) draws it on a host-supplied device. N4 connects them in its [spatial render pass](../engine/src/render/gpu/spatial.rs) and owns the timeline, native artwork and export. [nothing-spatial-cycles](cycles/README.md) is an optional bridge to Cycles for a scene that asks for a path-traced Final. [RFC 0009](../rfcs/0009-path-traced-final.md)

From `spikes/`, run the importer suite with the [pinned SDK](openusd/PROFILE.md#local-sdk-and-tests). The remaining two commands exercise a GPU directly and can run independently of the SDK.

```sh
cargo test --locked -p nothing-openusd --features require-sdk
cargo test --locked -p nothing-spatial-wgpu --test studio
cargo run --locked -p nothing-spatial-wgpu --example headless
```

## Coverage

Each row describes a particular stage. Importing a material, drawing its channels correctly and delivering an approved film look require different evidence.

| Capability | Reproducible evidence | Remaining boundary |
| --- | --- | --- |
| Rigid geometry and material assignments | The reader composes a reference, converts axes and units, preserves face material assignments and reports the source of import errors. Field Unit keeps addressable parts and material slots across its exported hierarchy. [Reader tests](openusd/tests/rigid.rs) · [Host tests](../engine/tests/usd_asset.rs) | Polygonal triangles and simple quads within the import profile. |
| Animation | The CPU reader can sample a stage at explicit USD time codes. N4 keys the recorder hinge around its authored pivot. [Hinge input](openusd/tests/data/hinge.usda) · [Shot](../examples/field-unit/sequences/instrument.xml) | Playback of arbitrary source-stage animation on N4 timeline time needs an explicit time mapping and reverse-seek proof. |
| UVs and surface maps | An indexed seam and packaged texture exercise different color and data interpretations of the same pixels. The renderer checks material channels and tangent normals. The N4 relight test restores the original frame after changing lights. [Reader tests](openusd/tests/rigid.rs) · [GPU tests](wgpu/tests/studio.rs) · [Relight test](../engine/tests/studio_relight.rs) | Opaque Preview Surface networks with supported sampling semantics. |
| Native artwork in 3D | Field Unit uses live video and lettering on a display, print on the case and frame-pinned captions. The standalone GPU example checks object coverage independently of project logic. [Project](../examples/field-unit/) · [GPU host](wgpu/examples/headless.rs) | Reflective coating over live display artwork remains open. |
| Path-traced Final | A scene with `path-trace` is drawn as a Draft by the rasterizer and as a Final by embedded Cycles, from the same rect lights. An emissive surface lights the scene. Caption edits re-trace nothing; artwork, light and pose edits re-trace once; export delivers stored pictures. [Receipt](cycles/verification/macos-arm64.json) · [Engine tests](../engine/tests/path_trace.rs) | Optional build feature with a provisioned Cycles bundle. Draft has no occlusion for reflections. Re-renders are within 2 of 255, not identical. One machine; no creative review. [Limits](../rfcs/0009-path-traced-final.md#limits) |
| Lighting and color | A distant key, filtered shadow map, analytic studio reflections, linear highlights and an SDR finish have GPU controls. [Studio suite](wgpu/tests/studio.rs) | Open for a positioned area light with matching highlight and penumbra, authored HDR environments, indirect light and a matched final-render quality review. |
| Deformation and larger scenes | Tests check diagnostic behavior for skinning and implicit subdivision. [Reader tests](openusd/tests/rigid.rs) | Stage cameras/lights, skinned geometry, subdivision, arbitrary networks and production-scale scene support need separate coverage. [Production-scale qualification](openusd/fixtures/README.md#production-scale-qualification) |
| Packaging | A standalone macOS arm64 reader can bundle its dynamic libraries, plugins and two original check fixtures. [Packaging and checks](openusd/PROFILE.md#standalone-macos-qualification) | The independent-host check requires running the two-host workflow. Distributing a signed editor and benchmarking full-film playback are separate gates. |

## Updating coverage

For each change, update the relevant row and its test. An importer change needs a small source input, a CPU assertion on normalized data or a specific diagnostic, and an update to the import profile. A visible feature needs a GPU check and a shot or edit through N4. A quality claim needs a reviewed frame range, reference and creative verdict. For playback, measure on the intended hardware and identify the scene, build and resolution. [Regression guide](../../docs/tests.md) · [Performance guide](../../docs/performance.md)
