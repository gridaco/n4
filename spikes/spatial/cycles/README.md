# nothing-spatial-cycles

A long-lived [Cycles](https://www.cycles-renderer.org/) session fed from `nothing-spatial` data. N4 uses it as the path-traced Final renderer for a shot that asks for one; the [wgpu rasterizer](../wgpu/src/lib.rs) remains the Draft and the Final for every other shot.

- [`native/`](native/n4_cycles.h): a C interface over Cycles' C++ scene and session classes.
- [`patches/`](patches): five lines that let the Cycles build compile `native/` as a shared library. Cycles' sources are otherwise untouched.
- [`src/lib.rs`](src/lib.rs): the Rust API. One session; only what changed is updated.
- [`examples/spike`](examples/spike/main.rs): a headless host for qualifying the bridge without the engine.

Pinned to Cycles `v5.3.0`. Cycles is Apache-2.0. Its C++ API carries no stability promise, so expect to revisit `native/` on an upgrade.

## Provision

The crate builds without Cycles; `nothing_spatial_cycles::available()` is then false and `Session::new` explains what to run. To render:

```sh
spikes/spatial/cycles/tools/provision.sh      # about 2.6 GB of downloads into .local/cycles
(cd spikes && cargo build --release -p nothing-spatial-cycles --examples)
```

`.local/cycles/install` then holds `libn4cycles.dylib`, its `lib/`, and `source/`, the kernel source Cycles compiles for Metal at run time. The three stay together. Set `N4_CYCLES_DIR` to use a bundle elsewhere. The first render on a machine compiles Cycles' GPU programs and took about three minutes on an M4 Pro; later starts take about a second.

## Check

```sh
python3 spikes/spatial/cycles/shots/field-unit-reflection/make_inputs.py
cd spikes
./engine/target/release/examples/spike calib --device METAL
./engine/target/release/examples/spike textures
./engine/target/release/examples/spike render --shot ../.local/cycles/shots/field-unit-reflection/shot.json \
  --frames 0..51 --samples 64 --denoise --masks --out ../.local/cycles/out/range
```

`calib` checks light units and the camera against quantities with known answers. [`reference/`](reference/blender_reference.py) renders the same shot file with Blender's own importer and Cycles for comparison.

## Settings worth knowing

- `CYCLES_CONCURRENT_STATES_MAX` (with `_BASELINE` and `_MIN`) caps Cycles' pool of in-flight paths. By default it grows to fit free memory: 5.7 GB on a 48 GB M4 Pro. At 1,048,576 it used 0.47 GB for 7% more time per frame.
- Denoising adds a fixed 1.4 s and about 2.9 GB of process memory per 1080p frame.

## Known traps

- Include `scene/shader.h` before `scene/background.h`. Otherwise `Background::set_shader` binds to a `bool` overload and the next update crashes.
- Without an OpenColorIO configuration Cycles converts 8-bit sRGB textures through a half-float path. The bridge decodes to linear floats itself.
- A rect light's strength with `normalize` off is radiance times pi.
- In-progress pictures arrive through `Monitor::snapshot` only from a session made with `Session::with_snapshots`. Cycles then denoises and reads back intermediate results, which cost about 0.6 s on a 64-sample 1080p frame, so the engine's Final renders do not ask for them. Cycles' viewport scheduling (`background = false`) never returns to a blocking caller; do not use it.
- `Session::get_progress` is declared by Cycles but not defined; use `progress.get_progress()`. A link failure here is easy to miss when filtering build output for compile errors.
- Re-renders are not bit-identical: on a 51-frame range no frame matched, though no pixel differed by more than 2 of 255 after the finish.
