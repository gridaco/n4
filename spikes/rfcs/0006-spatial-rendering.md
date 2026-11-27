# RFC 0006 — Shipping mixed 2D and 3D in N4

**Status.** The spike implements the rigid-product profile on macOS with a Rust host, an optional OpenUSD reader and a wgpu raster pipeline. The public Field Unit project exercises the mixed-space authoring contract. Production look approval, wider scene coverage and independent-host distribution qualification remain open. [Coverage tracker](../spatial/README.md) · [Field Unit](../examples/field-unit/README.md)

## What a creator should get

A solo creator brings a product model, composes its shot in the video timeline, lights it, puts live artwork on it, and exports the cut they approved. Changing the headline, slipping a screen recording or adjusting the camera should take an ordinary project edit. An externally modeled object can be the most expensive asset in the shot. Its hierarchy, pivots and material assignments are valuable authoring data.

The proposed v1 promise is **studio product motion for rigid assets**. A stage with shared visibility is a technical prerequisite. A convincing material, a light a designer can move and a shadow that grounds the product are part of the creative minimum. A broad promise to reproduce finished Blender scenes would also cover complex material graphs, deforming assets, transparency and indirect lighting. That promise changes the renderer selection and the schedule. Browser renderers already offer metallic/roughness materials and environment maps. Our advantage has to show up in the creator’s revision and the delivered film. After Effects offers a particularly relevant precedent for using live layers as material sources. [Three.js materials](https://threejs.org/docs/pages/MeshStandardMaterial.html) · [After Effects layer material sources](https://helpx.adobe.com/after-effects/desktop/work-with-3d-composition/work-with-materials/use-layers-as-material-sources.html)

For v1, **authoring a shot in N4** means placing and animating parts, choosing cameras and lights, binding editable artwork, directing the look, and cutting the result. Modeling the enclosure, UV unwrapping and rigging can stay in a DCC. A creator starting from an empty project can already supply procedural geometry through USD. A native extruded-type or spline tool would deserve its own authoring trial if building the object itself in N4 is part of the release promise.

### What already works

| Contract | Public proof |
| --- | --- |
| A camera sees a mesh and a live graphic in the same depth space. The host can use the occluded samples in its own post-process. | [Headless GPU example](../spatial/wgpu/examples/headless.rs) |
| The C++ reader composes references and exports polygonal geometry to Y-up meters, retaining the part tree, pivots, material slots and texture coordinates. | [Import profile](../spatial/openusd/PROFILE.md) · [Importer tests](../spatial/openusd/tests/rigid.rs) |
| A studio render uses linear floating-point color, a filtered shadow map and an SDR delivery transform. A native text or video edit can update artwork bound to a material. | [Studio shader](../spatial/wgpu/src/shaders/studio.wgsl) · [Spatial pass](../engine/src/render/gpu/spatial.rs) · [Instrument shot](../examples/field-unit/sequences/instrument.xml) |

## The first shot and the edits that decide it

Produce a six-to-eight-second studio spot of a hinged metal device with a recessed display. An artist supplies a real DCC export and a look reference. Start wide, open the lid about its authored pivot, then cut close enough to read two lines of live screen artwork. A bevel catches a moving highlight. The case casts a contact shadow on a matte ground. Part of the bezel passes in front of the moving screen image. A brand caption stays pinned to the output frame. Field Unit is the distributable example for this profile.

1. Change the model name in the live artwork and slip its video by three frames. The new artwork follows the screen, fits the recess, and stays correctly occluded in both cameras.
2. Retime the lid. Replace the housing asset with a supported revision. The pivot, label and shot cuts survive. A renamed material slot produces an actionable diagnostic.
3. Rotate the studio environment or key light. The case highlight and the shadow respond to their respective light sources. The display retains its authored brightness and the superimposed caption retains its delivery color.
4. Roll the cut one frame, reverse-scrub through it and export. Review at actual speed for crawling edges, unstable highlights and video judder. Compare the parked final frame with the encoder input. Repeat import and relighting on a held-out asset.

Agree on the stills **and the moving reference** before building to them. Artists judge the look; a pixel oracle judges geometry, timing and pipeline agreement. Either test can fail while the other passes.

## Ownership and rendering contract

Keep `project.proj` authoritative for editorial placement and cuts. A sequence owns local cameras, spatial instances and native artwork. A referenced USD stage owns its authored geometry, materials, internal transforms and source time. Each placement supplies its transform, time mapping and explicit overrides. Scope a binding by instance and USD prim path or material slot. Two placements of the same file can have different artwork. The ordinary N4 hierarchy remains the place to edit text, paths, video and effects. [Camera contract](../docs/research/video-editor/camera.md) · [USD and XML study](../docs/research/video-editor/usd-and-xml.md)

| Author’s intent | Renderer behavior |
| --- | --- |
| Pin a caption to the delivered frame | Draw in the 2D stack after the spatial group, in the intended output color context. |
| Put animated UI on the device screen | Evaluate an N4 detail at its mapped time into a GPU texture. Bind it to the display’s UVs as unlit artwork. It participates in visibility and receives the scene’s display transform. An unlit surface can appear bright without illuminating its neighbors. |
| Print a word on the metal case | Use artwork as a base-color input to the case material. The surface normal, roughness and lights govern its appearance. This can share the screen’s source-texture interface. |
| Float a cutout title in front of the product | Give a plane its own world transform. Covered samples participate in depth and object mattes. Empty letter counters reveal the scene behind them. |
| Apply a graphic treatment to a crossing | Finish the resolved scene in N4 with explicitly requested projected/visible/occluded mattes. Layers outside the spatial group see the finished picture. |

A flat composition can stay entirely on today’s fast path. A scene group is an explicit boundary for depth and lighting. Two assets that must intersect must enter the same scene before its color resolves. Cyclic dependencies such as a display sampling the scene that contains it need a diagnostic. A video on a plane has the plane’s depth; occlusion between actors *inside* the footage and a model requires a separately supplied matte, depth or geometry.

### Implementation boundary

Incubate the Nothing spatial libraries together under one physical directory in N4. Give USD coverage its own native build, fixtures and test command. N4 ships the first integration. An independent host can consume the importer while keeping its own renderer.

#### Repository and crate layout

The spike workspace builds four private packages with one toolchain and lockfile.

```text
n4/spikes/
  Cargo.toml, Cargo.lock, rust-toolchain.toml
  engine/                           n4-engine: existing product host
    src/                            master, XML, timing, cameras, text, media, compositor, transport
    native/video_decoder.c          FFmpeg shim
    src/render/gpu/spatial.rs       N4 shot → spatial draw inputs
    tests/                          USD fixtures, 2D regression, preview/export integration
  spatial/                          one independently movable source directory
    scene/                          nothing-spatial: source-independent CPU data
      src/lib.rs                    rigid hierarchy, pivots, UVs, material slots and decoded image data
    openusd/                        nothing-openusd: optional native USD adapter
      build.rs, native/             SDK discovery/linkage and C++ bridge
      src/                          Rust API for loading a selected prim and reporting failures
      fixtures/, tests/             small USD inputs, expected semantics and rejection cases
      PROFILE.md                    tested SDK, supported properties, limits, remedies
    wgpu/                           nothing-spatial-wgpu: GPU renderer
      src/                          uploads, resource handles, pass encoding and built-in WGSL
      examples/headless.rs          independent GPU host, camera and coverage control
      tests/studio.rs               studio lighting, color, slot binding and visibility controls
  examples/                         canonical user productions and their own WGSL
  sdk/python/, rfcs/, docs/         authoring SDK, proposals and research
```

Both the USD adapter and the GPU renderer depend on the small CPU crate. They are siblings and build independently. N4 connects them and supplies the evaluated camera, time and live textures. The USD adapter can run on a machine with the SDK and no graphics device; the headless GPU host can render a procedural mesh on a machine without the USD SDK. A normal 2D N4 build remains possible without provisioning USD. Keep one Cargo workspace and lockfile during incubation, preserve the existing launcher/SDK binary lookup, and give the moved bridge an explicit SDK location. Build and test an actual final executable outside the development prefix to verify native library and plugin discovery. [Current build](../engine/build.rs) · [SDK packaging](../engine/native/OPENUSD.md)

**Why the CPU crate earns its place.** The importer returns owned mesh data. N4 associates it with its source and selected prim at project open or edit preparation. The draw pass uploads a prepared mesh on first use and reuses those GPU buffers across frames and cameras. A procedural host can construct the same mesh in memory. The Blender trial added UVs, addressable parts, authored pivots and stable material slot paths to that boundary. [CPU types](../spatial/scene/src/lib.rs) · [N4 draw path](../engine/src/render/gpu/spatial.rs) · [Asset preparation](../engine/src/graphics.rs)

**Recurring USD work.** Each supported feature lands with a tiny independent asset, a CPU assertion on the normalized result or exact diagnostic, and a row in the profile. For face-varying UVs, test a shared position with different UVs on adjacent faces; for an animated hinge, test two local poses, parent composition and the retained pivot. The adapter owns accurate translation. When a feature changes the picture, add an independent GPU control and an N4 frame check before marking it supported end to end. Report ingestion and rendering coverage separately. Run the importer suite with a required, pinned SDK on each change; a skipped native suite cannot qualify a release. Test the SDK-free renderer separately and trial new SDK versions against the same corpus before changing the pin. This is a read/import contract; USD authoring and round-trip export would require their own fidelity proofs.

**Independent host qualification.** Import one 10 cm USD part next to a native 10 cm cube. Check units, orientation, material and the part pivot. To qualify the renderer separately, draw the host mesh and imported part intersecting on the same device. Sharing depth requires both passes to agree on projection, clear value, depth comparison and sample count. The N4 spatial pass uses reversed depth. [N4 spatial pass](../engine/src/render/gpu/spatial.rs)

The Field Unit shot already exercises part and material edits in N4. Publish the spatial libraries separately once a second host consumes a pinned revision and benefits from independent releases.

The host evaluates the frame and calls `Renderer::record` with its encoder, color texture, optional matte output, sampled view, geometry and lights. It owns queue submission, presentation and any readback. The renderer reuses its pipelines, multisampled color and depth, and matte scratch. The host receives single-sampled sRGB color and optional R8 matte layers. The studio pass uses an internal linear RGBA16F target, a directional shadow map and an SDR finish. A constant-material scene can use the simpler direct-light finish. Moving a camera updates view data; replacing one detail texture updates that binding. A clean frame evaluation must work at any requested time, including after a backward seek.

Preserve persistent mesh, texture and material handles separately from instances, evaluated transforms and per-frame content revisions. Keep prim paths and dependency keys for revision, diagnostics and editor selection. Native artwork and imported assets enter the same draw/visibility machinery after preparation. WGSL may share UV, noise and pattern functions across passes. A surface program supplies documented material inputs; an image filter reads a bounded image neighborhood; an object-matte filter receives named screen-space masks. A future ray integrator can evaluate portable material inputs or report unsupported programmable behavior. Screen-neighbor reads, derivatives and frame history need their own execution contracts. Arbitrary WGSL portability across all three would be a misleading guarantee.

### Proposed raster, color and USD profile

1. Ask OpenUSD to compose the selected subtree and sample the supported rigid transforms at the mapped asset time. Preserve hierarchy, normals, face assignments and UV seams. Reuse the existing C++ bridge and wgpu raster pipelines. N4 owns texture binding, scene evaluation for the shot, lighting policy, object mattes and integration with the video compositor.
2. Prepare live artwork at an explicit resolution appropriate to its projected size, including the macro shot. Cache by detail revision, time, size and color space. Treat base-color textures, linear material data and video transfer functions explicitly. Shade into scene-linear floating-point color (proposed RGBA16F, specified working primaries), keep straight material inputs distinct from premultiplied compositing outputs, and apply a documented view transform once. Test two highlights above 1.0 remaining distinct through exposure adjustment. SDR delivery is sufficient for the first shot.
3. Start with opaque and coverage-masked geometry, textured base color, roughness/metallic, a controllable environment for specular highlights, and one shadow-casting key with bias/filter controls tested in motion. A fixed shadow map is an acceptable first implementation for the qualified scene. Keep direct light, environment reflections, inter-object reflections and bounced light as separate promises. General translucent intersections, refractive glass and participating volumes require different visibility and shading rules. A 2D glass treatment samples the composed image and can use the existing backdrop-effect path.
4. Pin the tested OpenUSD SDK and enumerate the admitted profile. Start with local USDA/USDC tested separately, composed local references, selected visible `Xform`/`Mesh` prims, rigid transforms/pivots, polygon meshes (`subdivisionScheme=none`), supported triangulation, supported normals/UV `st` interpolation and material subsets. Add a tested `UsdPreviewSurface` texture-reader/UV-reader path for the actual product. Declare the interpolation, time-code conversion, face orientation, unit and up-axis rules. Keep a regression for opaque constant materials alongside the textured case. Stock Preview Surface describes material inputs; the studio lighting and view transform also determine the picture. [Mesh schema](https://openusd.org/release/api/class_usd_geom_mesh.html) · [Preview Surface specification](https://openusd.org/release/spec_usdpreviewsurface.html) · [Blender USD export](https://docs.blender.org/manual/en/4.2/files/import_export/usd.html)
5. Publish a table of tested, intentionally ignored and rejected schema features. Report asset, selected prim, property/connection and remedy before final rendering. A mesh with the default Catmull–Clark subdivision scheme needs subdivision support or a polygonal export. An unsupported shader network, missing texture or deforming mesh must give a creator a clear choice. A labeled draft stand-in is useful for navigation. Gate clean-machine distribution of the reader and plugins; the current development SDK prefix is around 1.1 GiB. [Current importer](../spatial/openusd/native/usd_mesh.cpp) · [SDK installation](../engine/native/OPENUSD.md)

## Interaction and a future final renderer

Maintain one evaluated shot. During a drag, a labeled **Draft** can use bounded work, a lower render scale or cached material inputs. At rest the inspector renders **Final** with the same scene, time, settings, resources and renderer as export. A stale picture keeps its actual frame identity. An outline may help select objects during manipulation, but approval and color judgments require Final. During continuous playback, qualify a named quality mode at the film’s cadence. An export’s quality setting is reproducible independently of how long the last preview stayed on screen.

Use the existing latest-request transport. Tag work with revision, frame/subframe, camera, resolution and quality. Track dependencies through asset bytes, material bindings, nested detail revisions and video sample time. An edit to a caption after the scene can reuse its color; a changed model texture must invalidate the lit result even when its file path is identical. On an obsolete request stop scheduling new work, bound GPU work in flight and suppress stale presentation. Submitted GPU work may complete. Defer progressive accumulation until there is a backend with useful incremental samples. Such a backend also needs a reproducible sample sequence, an export stop rule, and accumulation reset on changed inputs. [N4 transport](../engine/src/transport.rs) · [WebGPU queue](https://www.w3.org/TR/webgpu/#gpuqueue)

A path tracer remains an option. Preserve geometry, material meaning, transforms and radiometric range now. Before promising inter-object reflections, soft indirect light or DCC look fidelity, compare a mature renderer such as Cycles against extending N4. A cheap evaluation can render the same shot through Blender offline, binding a frame of N4 artwork as a texture. Score the material translation, time to first approved frame, update cost after a copy edit, and ability to return the required mattes. That experiment can reveal whether integration is worth its cost; an exported movie alone cannot verify live revision. Hydra can connect scene descriptions and renderers; it supplies neither a guaranteed look nor automatic wgpu texture interoperability. If broad Cycles-class scenes are mandatory for the first release, run that backend selection **before** funding the raster expansion. [Hydra](https://openusd.org/release/api/hd_page_front.html)

## Milestones and release gates

| Gate | Evidence and next qualification |
| --- | --- |
| A. Reader and independent GPU host | Open a composed reference and inspect units, pivots, material roles and rejection reasons on CPU. On GPU, intersect a mesh and a host-supplied graphic, composite a matte and check two camera views in a caller-owned texture. [Import suite](../spatial/openusd/tests/rigid.rs) · [Standalone GPU host](../spatial/wgpu/examples/headless.rs) |
| B. Studio asset and surface edits | The public Field Unit model imports 29,928 triangles, 229 rigid parts and ten material slots. Its lid is independently keyed and its display and nameplate carry live artwork. A separate textured card exercises relighting and a reverse seek. [Field Unit source](../examples/field-unit/author/device.py) · [Hinge and slot tests](../engine/tests/usd_asset.rs) · [Relight test](../engine/tests/studio_relight.rs) |
| C. Studio look | Review the transition from opening product shot to moving screen close-up and the final wide. Exercise bevel highlights, shadow stability, display legibility and pinned titles. Engineering controls cover light and shadow behavior. Target art direction and creative approval require a separate review. [Field Unit](../examples/field-unit/README.md) · [GPU controls](../spatial/wgpu/tests/studio.rs) |
| D. Playback and revision | Measure first picture after open, source preparation, active scrub preview and exact parked frame after release. Move the source following preparation, reject an invalid part edit and verify the last accepted picture and mesh uploads. [Snapshot regression](../engine/tests/usd_asset.rs) · [Performance guide](../../docs/performance.md) |
| D. Reader distribution | Repackage the standalone arm64 reader, hide the development SDK and import composed USDA, product USDC and textured USDZ. The local receipt checks all three. An independent installation requires the manual build/consume workflow. [Local receipt](../spatial/openusd/verification/macos-arm64.json) · [Packaging](../spatial/openusd/PROFILE.md#standalone-macos-qualification) · [Independent-host workflow](../../.github/workflows/usd-reader-macos.yml) |

### Studio film and qualification

A contributor can change the Field Unit display title, slip the video, change a hinge key and reframe the shot using the checked-in assets. The [fixture catalog](../spatial/openusd/fixtures/README.md) distinguishes tiny source-level controls from the full product spot. The [coverage matrix](../spatial/README.md) tracks limitations and the responsibilities of each layer.

For look approval, compare an independent offline beauty frame and a short move against the same camera, product and lighting intent. For same-build regression, compare exact pixels. For lighting across devices, state the numerical tolerance. Record exposure, output color transform and hardware with each result. Unsupported subdivision, MaterialX, deformation, indirect lighting and production-scale scene handling need new proofs before extending the profile.

### Milestone A verification

The pinned importer requires the SDK and can run on the CPU alone. The GPU example uses programmatically generated geometry and runs independently of the SDK. Run these from `spikes/`. [SDK setup](../spatial/openusd/PROFILE.md#local-sdk-and-tests)

```sh
cargo test --locked -p nothing-openusd --features require-sdk
cargo test --locked -p nothing-openusd --no-default-features
cargo run --locked -p nothing-spatial-wgpu --example headless
```

**Proposed performance target.** At 1080p24 on the M4 Pro, aim for all 144 pictures presented on time on a warm six-second run and p95 release to the exact frame within two film frames (83 ms) after bidirectional drags. Report the distinct correct pictures shown during drag, worst hold, p95 pointer-to-picture lag, cold-open latency and peak GPU memory. Establish a measured baseline with the public project and repeat after changes to media, lights or camera cuts. [Regression guide](../../docs/tests.md) · [Performance guide](../../docs/performance.md)

If the release requires accurate inter-object reflections, transmissive surfaces and bounced light in look-developed stages, move the offline-renderer comparison forward and measure its cost on an actual edit.
