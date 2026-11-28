# Engine performance

A scrub can land on the right frame when the pointer stops while showing a frozen picture throughout the drag. Measure the frames a creator sees during the gesture, the time to the exact release frame, and whether the picture is correct. Use the same project, frame targets and quality settings across comparisons.

## Choose a workload

| Workload | Question | Input |
| --- | --- | --- |
| Edit a shared graphic on a parked frame | Which pictures, thumbnails and audio buffers survived the revision? | [Signal Atlas](../spikes/examples/signal-atlas/README.md) and the headless `just test-invalidation` regression |
| Scrub a 2D timeline with expanded layers | Does the picture follow the same targets with the inspector open? | [Paired SDL and Electron gesture runner](../spikes/host/test/compare-scrub.py) on Signal Atlas |
| Reverse direction across a video keyframe | How much codec preroll is required, and is the displayed picture correct? | [Variable-frame-rate screen capture and proxy](../spikes/engine/fixtures/README.md) |
| Play while thumbnails and proxies are being generated | Do picture deadlines or audio callbacks slip under load? | [Generated 4K long-GOP workload and device runner](../spikes/engine/fixtures/playback/README.md) |
| Switch views on an animated 3D product shot | Which meshes and material surfaces are uploaded again? How long does the exact frame take? | [Field Unit](../spikes/examples/field-unit/README.md) with the OpenUSD SDK |

## Measure the interaction

Record the project revision, build profile, OS, hardware, render size, display refresh, timeline rate, source codec, quality level and enabled background jobs. Name the start and end of each timed interval. A CLI render through RGBA readback, a frame submitted to a window and a physical display refresh have different end points.

- **Open.** Measure project open to first picture and to usable transport. Report media indexing, waveform and thumbnail work still outstanding.
- **Scrub.** Use one monotonic clock for pointer input, playhead position and presented frame identity. Drive input independently of rendering. Include slow motion, a fast sweep, reversal and release. Report distinct pictures, p95 distance from the requested frame, the longest picture hold and time to the exact release frame. A slow pointer can request the same source frame on several consecutive ticks.
- **Playback.** Compare the presented frame with the audio clock. Record missed and consecutive deadlines, audio starvation, drift and the longest hold. Cross a cut immediately after an expensive frame to catch stale completions.
- **Seek.** Report first visits and revisits separately. Include reverse steps immediately before a keyframe, distant jumps, and a working set larger than the cache. Compare picture IDs with a linear decode or capture oracle.
- **Memory.** Record peak and settled memory after repeated play, scrub and reopen cycles. Include decoder references, in-flight surfaces, thumbnails and GPU allocations where measurable.

## Diagnose the expensive stage

### Decode and presentation

For interframe footage, changing one frame in reverse can require decoding from an earlier random-access point. A stream with reordered packets can have a key packet’s decode time before the requested picture’s presentation time. Use the indexed group of pictures and validate the exact requested timestamp at original and proxy sizes. Compare an all-intra copy of the same pictures when isolating decode cost. [Decoder](../spikes/engine/src/media/native.rs) · [Timing regression](../spikes/engine/tests/proxy.rs)

Animated GIF and WebP replay earlier patches and disposal operations on a cold reverse seek. The optional `measure_animation_reverse_reads` test takes a file via `N4_ANIMATION_SAMPLE`. It requests each displayed source frame once in descending order, starting with an empty cache. It measures CPU decoding and retained canvas and output bytes. Encoded source bytes, transient allocations, composition and screen presentation fall outside this measurement. [Media measurement](../spikes/engine/src/media/tests.rs)

| Source on Apple M4 Pro, 640×360, debug build | Validation and open | Reverse seek p95 | Frames decoded | Peak cache and cursor charge |
| --- | ---: | ---: | ---: | ---: |
| WebP, 72 images | 11.31 ms | 8.50 ms | 2,628 | 58.01 MiB |
| GIF, 48 images with partial restore-previous patches | 36.39 ms | 23.31 ms | 1,176 | 43.41 MiB |

The GIF trial decoded 48 + 47 + … + 1 images. A warm interactive drag can revisit a cached canvas and avoid that work. If distant reverse seeks miss the target frame budget, measure whether caching periodic reconstruction states pays for the extra memory. A saved state must include pending disposal and the prior pixels needed for restore-previous.

For multiple video layers, record the active sources, decode sizes and trims. Track decoder opens and the combined memory across cursors. Compare repeated visits to a trim, simultaneous uses of the same source at different times, and a cut to zero active video.

A 1920 × 1080 RGBA8 picture occupies 8,294,400 bytes per full-frame copy. Time decode, conversion, upload, compositing, readback and presenter submission separately before changing the transport. Compare hosts in foreground windows at the same scale. Include thumbnail activity and the number of outstanding frame leases. [Host contract](../spikes/host/README.md)

### A small edit that redraws a scene

Compare the reopened project against a fresh open at the same frame. In Signal Atlas, grading the shared survey image affects two disjoint V1 intervals. Sample both, a warm frame in the gap and the independent V2 badge. Count actual renders, decoder requests and audio preparations. Editing only the reprise’s audio gain should change its waveform and preserve the picture. A formatting-only save should preserve the 2D thumbnails. [Executable comparison](../spikes/host/test/selective-invalidation.mjs)

For text, distinguish shaping, raster paint and GPU upload. Recoloring a span can reuse shaped glyph positions. Changing the width can reflow the paragraph and move a linked decoration. For vectors, compare a transform-only edit with a change to local path geometry. Confirm the returned pixels as well as the cache counters. [Text layout](../spikes/engine/src/graphics/text_layout.rs) · [Transform tests](../spikes/engine/tests/transform_group.rs)

### Source blur and a moving focus band

Compare the sharp layer, full-quality blur and an animated focus band on the same input. Measure the first request, a warm frame, consecutive decoded source pictures and a reverse visit. A moving band over a still can reuse the convolution. A changed radius, source revision or video frame changes its inputs. Verify sampling at the mask edge under camera zoom. Backdrop blur has a different dependency: every contributing layer already painted behind the mask. A sibling moving behind the glass can invalidate its result. [Source blur](../spikes/engine/tests/source_blur.rs) · [Backdrop](../spikes/engine/tests/backdrop.rs)

### Temporal effects and spatial shots

A history effect can need several source pictures for one output. Exercise cold and out-of-order seeks and verify the effect clears at a cut. Record additional decode requests, upload count, passes and texture memory. The cost grows with render dimensions and lookback.

For a 3D scene, separate stage import, material preparation, scene rendering and final composition. Alternate two camera poses while holding the model fixed. Mesh buffers should remain reusable. Remove an object-matte filter in the control shot so that its declared inputs and additional coverage passes disappear. Compare warm timings and scratch allocation at the same device resolution. [Spatial coverage](../spikes/spatial/README.md)

## Qualifying a change

Keep the exact command, source fixture, cold-start conditions and raw trace with the result. Compare at least one ordinary interaction and the suspected worst case. Report the median, tail, slow frame or gesture and correctness check. For a small number of releases, give their individual times. Repeat on the same machine before interpreting a difference as an improvement. [Regression tests](tests.md)
