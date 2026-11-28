# RFC 0009 — A path-traced Final for spatial scenes

**November 28, 2026 · Implemented on `cycles-final` and qualified on one machine.** A `scene3d` can ask for a path-traced Final. The rasterizer then draws its Draft, and [Cycles](https://www.cycles-renderer.org/) v5.3.0, embedded as an optional library, traces the picture a creator approves and exports. A shot that does not ask keeps the rasterizer as its Final and behaves exactly as before. [Qualification receipt](../spatial/cycles/verification/macos-arm64.json) · [Bridge](../spatial/cycles/README.md) · [RFC 0006, a future final renderer](0006-spatial-rendering.md#interaction-and-a-future-final-renderer)

## What a creator gets

Three kinds of picture the rasterizer cannot make: a softbox whose size shapes both highlight and shadow edge; parts that light and mirror each other, including a screen whose live artwork glows on its surroundings; and occlusion in those reflections. The creator still edits text, footage, cameras and cuts in N4, and those edits reach the traced picture.

1. **Scrub a Draft.** Any drag or playback of a path-traced scene shows the rasterizer's picture from the same lights. It lacks the screen's light, bounced light and occluded reflections.
2. **Stop on a frame to see Final.** The Draft appears at once and the Final replaces it when traced: about 4.5 s for a 1080p frame at 64 samples with denoising on an M4 Pro. The viewer's title says which is on screen.
3. **Finish a range, then review it at speed.** `just finish <project> --frames 0..51` traces a range once. The viewer then plays the stored pictures.
4. **Export what was approved.** Export feeds the encoder the stored Finals and traces any that are missing. A build without the path tracer fails rather than delivering a Draft.

## Authoring

```xml
<scene3d pixels-per-meter="3200">
  <studio shadow-center="0 0.061 0" shadow-extent="0.39" shadow-bias="0.0003" shadow-softness="4"/>
  <path-trace samples="64" bounces="8" clamp="10" denoise="true" seed="0"/>
  <ambient-light intensity="0.05"/>
  <rect-light center="-0.36 0.52 0.34" aim="0 0.05 0" size="0.5 0.3" intensity="11" color="#fff0dc"/>
  <usd id="recorder" source="assets/field.usdc" prim="/World/Recorder">
    <surface slot="/World/_materials/Display" role="emissive" glow="4" size="640 360">…</surface>
  </usd>
</scene3d>
```

- **`rect-light` and `ambient-light` are lights any renderer can reproduce.** The analytic studio environment is a set of constants in the rasterizer's shader, so a scene with a rect light is lit by its rect lights and ambient term alone. Both renderers read the same values; no per-shot matching is involved.
- **`role="emissive"` is a screen.** It is shown as authored times `emission`. In the Final it lights and reflects in the scene, scaled by `glow`, which is an art control: at a physical 1 a screen is faint beside a key light.
- **`path-trace` is the recipe.** Preview, range rendering and export all use it. Nothing about a picture depends on how long a preview stayed on screen.
- **Scope.** A path-traced scene holds USD models. Distant lights, text planes, and projected or occluded-by mattes are rejected there with a diagnostic; visible-coverage mattes are supplied.

## Contract

**A finished picture is identified by content, not by location.** The key hashes the authored scene with every file it names replaced by a digest of that file's bytes, the scene's frame and sub-frame, the camera, the output size, the imported geometry, materials and textures, the requested mattes, the Cycles device, release and bridge revision. A surface that shows video, a wipe or a periodic shader is also keyed on the shot's clock, because those are sampled on it. [`final_key`](../engine/src/render/gpu/spatial.rs)

The key covers the scene as authored, so it errs toward tracing again: a keyframe changed elsewhere in the scene re-traces frames it does not affect, and sliding a scene whose screen shows video re-traces it.

| Concern | Rule |
| --- | --- |
| Draft or Final | The spatial pass looks the key up. A stored picture is composited as the group; otherwise the rasterizer draws and a request is left behind. `RenderTrace.graphics` counts `path_traced_final` and `path_traced_draft`. |
| Tracing | `Engine::render_final` resolves the requests: it reads back each live surface, hands one evaluated instant to the path tracer, applies N4's film curve on the CPU and stores the group. It then renders again. |
| Invalidation | An edit outside the scene leaves the key unchanged and re-traces nothing. New artwork, a moved part, light or camera is a new picture, and so is footage or an image replaced on disk under the same name. A copy of the project elsewhere finds the same pictures. |
| Sample pattern | The seed follows from the recipe and the key, never from render order. |
| Persistence | Hosts keep pictures in the user cache (`~/Library/Caches/n4/final`, or `N4_FINAL_STORE`). Keys describe content, so a copy of a project finds the same pictures. The cache holds 20 GB (`N4_FINAL_STORE_GB`); beyond that the pictures used longest ago are deleted and would be traced again. A directory named with `--final-store` is never trimmed. |
| Interaction | The transport follows a parked Draft with its Final. Moving on, an export and its cancellation each stop a trace in flight, including one that has not started sampling. Pausing or releasing a drag on a Draft asks for that frame's Final. Draft pictures are not kept in the composed-frame cache. |
| Colour and alpha | Cycles returns scene-linear premultiplied colour on a transparent film. The group handed to the 2D stack has the same format as the rasterizer's. |

The engine's path tracer sits behind [`render/path_trace.rs`](../engine/src/render/path_trace.rs), which takes an evaluated request and returns a picture. Nothing above it names Cycles. A tracer written against wgpu could replace it there; that is a separate project and is not planned here.

## Evidence

One machine: Apple M4 Pro, macOS 26.6, 1080p, Cycles on Metal. [Receipt](../spatial/cycles/verification/macos-arm64.json) · [Script](../spatial/cycles/tools/qualify.py)

| Check | Result |
| --- | --- |
| Final traced once per frame | 3 of 3 frames traced; 4.5 to 5.6 s each |
| A second process, reverse order | 0 traced; pictures byte-identical |
| A relative path; a copy of the project | 0 traced |
| Cold re-render, reverse order | Largest difference 1 of 255 |
| Caption edit | 0 traced; 0.2 s a frame |
| New word on the screen | 3 traced; 9,657 pixels changed on frame 4, 1,185 of them below the lid |
| Footage slipped three frames | 3 traced |
| Key light moved | 3 traced |
| Range, then export | 51 frames traced in 225 s (4.4 s median); export 1.1 s; a second export fed the encoder the same bytes |

The receipt backs the table above. The other figures in this document (the Blender comparison, the Draft's match on matte surfaces, the 51-frame repeat, held-frame noise, memory and first-start time) were measured once in a standalone study with this bridge on the same machine; nothing tracked reproduces them yet.

In that study the bridge rendered this shot from the importer's data and was compared with Blender 5.3's own Cycles reading the same USD and light rig: mean difference 0.83 of 255 at 4096 samples, brightness within 1.4%. Engine tests cover the Draft count, single tracing, the edits above on a small frame, export, and the transport's Draft-then-Final. [`tests/path_trace.rs`](../engine/tests/path_trace.rs) · [Bridge tests](../spatial/cycles/tests/bridge.rs)

## Limits

- **Draft does not predict reflections.** With rect lights the rasterizer matches Cycles on matte surfaces (a floor within 4 of 255). Glossy parts that should mirror something between them and a light mirror the light instead. Judge light placement and exposure on Draft, reflections on Final.
- **Re-renders are near, not bit, identical.** On a 51-frame range no frame matched exactly and no pixel differed by more than 2 of 255. Export therefore reuses stored pictures.
- **Frame-to-frame noise.** On held frames the best tested recipe kept 99.2% of pixels within 2 of 255. Cycles has no temporal denoiser on macOS. No one has yet judged a loop by eye.
- **First start.** The first render on a machine compiles Cycles' GPU programs: about three minutes on the M4 Pro, then about a second. What invalidates that cache is not characterised.
- **Memory.** The bridge caps Cycles' path pool at about a million paths (0.5 GB). Denoising adds about 2.9 GB at 1080p.
- **Cycles' C++ API is undocumented and unstable.** The bridge is pinned to v5.3.0; an upgrade needs re-qualification.
- **Not built:** in-progress pictures in the viewer (the bridge can deliver them, at a cost of about 0.6 s a frame); path-traced text planes and distant lights; projected mattes; signing of the Cycles bundle; any machine other than the one above.

## Build

Cycles is optional and is not downloaded by an ordinary build.

```sh
spikes/spatial/cycles/tools/provision.sh                    # about 2.6 GB into .local/cycles
(cd spikes && cargo build --release -p n4-engine --features cycles --bin n4)
python3 spikes/spatial/cycles/shots/field-unit-reflection/make_project.py
just finish .local/examples/field-unit-reflection
N4_CYCLES=1 just inspect .local/examples/field-unit-reflection
```

The provisioning script replaces Cycles' precompiled image library, which statically includes LGPL-3 code, with a build that has none. The bundle's notices still need review before any release.
