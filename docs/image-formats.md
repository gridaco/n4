# Primary raster image import

**Ratified for authored 2D media.** A creator can place a photograph, graphic or animated image directly in a project. Stills use `<image>`. A single file with an animation uses the same placement, source trim and playhead as other footage.

| Source | Authored as | Current engine |
| --- | --- | --- |
| PNG still | `<image>` | Supported through Skia, including transparency. |
| JPEG still (`.jpg`, `.jpeg`) | `<image>` | Supported through Skia, including baseline, progressive and grayscale photos. JPEG is opaque. |
| WebP still | `<image>` | Supported by N4’s Rust reader, including lossy and lossless images with alpha. |
| Single-image GIF | `<image>` | N4 decodes the logical canvas, including palette transparency and partial placement. |
| Animated WebP | `<video>` | Supported in the Rust engine via libwebpdemux. Variable holds and transparent partial frames are seekable. |
| Multi-image GIF | `<video>` | N4 reconstructs each displayed canvas, including restore-previous. The result is seekable footage with alpha. |
| APNG | Future candidate | Deferred. A PNG reader can display the default image. Animated playback requires separate qualification. |

### Authoring and recognition

```xml
<image name="Photo" source="assets/portrait.jpg" position="96 48"/>
<video name="Pointer" source="assets/pointer.webp" start="12" source-in="0" duration="7" size="160 96"/>
```

The second line places a seven-project-frame excerpt. The same placement accepts an animated GIF by changing the source path. Use `<sequence source="assets/pointer.webp"/>` for primary footage. Its uncovered pixels composite over the output black. An independent layer shows the artwork beneath it through the same pixels. A clip contributes no audio. Animated GIF import as one piece of footage has precedents in [After Effects](https://helpx.adobe.com/after-effects/desktop/get-started/supported-file-formats/supported-file-formats.html) and [Premiere](https://helpx.adobe.com/premiere/desktop/organize-media/import-files/import-images-as-image-sequence.html).

Classify the asset by its contents. For GIF, count image descriptors. A loop extension alone does not make a single image timed footage. For WebP, recognize its animation container, even with one frame. Opening either kind of animation as a still must identify the asset and direct the author to `<video>`. APNG may have a `.png` suffix and its default image may sit outside the animation. Any future reader must check the PNG animation control chunk (`acTL`). [WebP container](https://datatracker.ietf.org/doc/html/rfc9649) · [GIF89a](https://www.w3.org/Graphics/GIF/spec-gif89a.txt) · [PNG specification](https://www.w3.org/TR/png-3/)

### Playback contract

- **Time and trims.** One pass through the asset defines its source duration. Keep the embedded repeat count as metadata. Authors can repeat the placement in the edit. Compare rational source time with half-open display intervals. Keep every positive delay, including short holds. Project ticks sample whichever source frame covers that instant. Existing source-in and duration limits apply.
- **Missing or zero delays.** GIF expresses delays in hundredths of a second. N4 assigns a 100 ms hold to a GIF image with zero or absent delay. This editorial default lets untimed multi-image GIFs play deterministically. Reject interactive GIFs that request user input. WebP expresses durations in milliseconds. Apply a zero-duration WebP frame’s blend and disposal to the canvas without displaying an interval. Reject a WebP animation with zero total duration.
- **Displayed pixels.** Decode at the logical canvas size. Preserve the displayed image before disposal, including partial-frame offsets. A transparent GIF palette entry leaves the destination pixel in place. Respect local palettes, interlace, keep, restore-background and restore-previous. For GIF overlay import, initialize the canvas and clear disposal rectangles to transparency when the corresponding first or disposing image declares transparency. Otherwise use the valid global background color, falling back to transparent if unavailable. WebP uses replace or source-over and a transparent initial and disposal background. Apply the still WebP color contract and carry alpha through resizing and compositing. The two codecs deliver the same complete frame representation to the renderer. [WebP color and alpha](../spikes/rfcs/0010-webp-import.md)
- **Seeking.** A direct seek, forward play and a reverse scrub to the same source instant must yield the same pixels. Index timings once per accepted source revision. Reuse sequential decoder state and cache displayed frames within a byte budget. On a cold miss, replay from the start or from a checkpoint containing the state needed for the next frame. Bound input, frame count, decoded allocations and retained decoder state before accepting a source.

### Decoder ownership and qualification

N4 recognizes GIF and WebP by signature and bounds the encoded input before selecting a decoder. Each animation adapter validates the source and reconstructs full-size straight RGBA8 frames. N4 owns the display clock, source trims, resizing, worker-local seek state and cache budget for both codecs. [Animation reader](../spikes/engine/src/media/animation.rs) · [Still reader](../spikes/engine/src/media/still.rs)

For WebP, the still and animation readers share ICC conversion inside the media module. WebP stills use `image-webp`. For animation, libwebpdemux returns a reconstructed straight RGBA canvas. N4 converts tagged RGB to sRGB before composition. The RGBA output matters even when the file omits the alpha flag. On the 4×2 patch control the Rust decoder's RGB animation output discarded coverage beyond opaque patches. Google libwebp retained it. [Still reader](../spikes/engine/src/media/webp.rs) · [Animated reader](../spikes/engine/src/media/webp/animation.rs) · [Canvas assembly](https://datatracker.ietf.org/doc/html/rfc9649#section-2.7.2)

For GIF, the Rust `gif` crate reads indexed pixels, palettes and frame controls. N4 treats the palette triplets as sRGB and applies offsets, transparent indices and disposal in display order. A restore-previous frame saves the pixels it will overwrite so the next frame can recover the canvas that preceded it. The still importer uses the same canvas rules for its single image. The format adds no native SDK dependency. [GIF adapter](../spikes/engine/src/media/gif.rs) · [Decoder](https://docs.rs/gif/0.14.2/gif/struct.DecodeOptions.html)

The engine checks the full compressed stream when accepting a revision. Workers share the immutable bytes and exact display index. Each worker opens its own cursor for a source and output size. Its frame cache shares a byte budget across profiles. On a reverse miss the cursor resets and decodes from the beginning. Retained codec working memory counts toward the cache budget even for a tiny thumbnail. A larger cursor may be used transiently and released after the read. Animated image previews use the original footage. The compositor converts straight color to linear premultiplied samples before filtering. A source thumbnail preserves straight alpha. A composed thumbnail shows the finished picture. [Media profiles](../spikes/engine/src/media/assets.rs) · [Seek and cache](../spikes/engine/src/media.rs) · [Compositor](../spikes/engine/src/render/gpu.rs)

**WebP control.** The original 4×2 control draws three colored 2×2 patches held for 40/80/20 ms. At 50 fps the display intervals are `[0,2)`, `[2,6)` and `[6,7)`. Seek backward through a disposal boundary over a moving background and compare every pixel with independent still witnesses. Check raw and composed thumbnails, a transparent primary shot, the encoder's input hash and a cold worker after rejecting a corrupt replacement. The codec and compositor suites also exercise fractional rates, zero-duration frames, ICC conversion, resizing, translucent edges, source blur and history. [Fixture generator](../spikes/engine/fixtures/webp/generate.py) · [End-to-end checks](../spikes/engine/tests/animated_images.rs) · [GPU checks](../spikes/engine/src/render/gpu/timed_alpha_tests.rs)

**GIF control.** On an 8×4 canvas, a temporary green patch overwrites red and covers part of a transparent area. When blue appears elsewhere, the green must disappear, revealing both the original red and the moving plate through the restored hole. The following yellow frame must also expose the plate where blue cleared. The four holds are 40, 60, 20 and 80 ms. At 50 fps, their onsets are frames 0, 2, 5 and 6. Read across both boundaries in either direction. [Fixture generator](../spikes/engine/fixtures/gif/generate.py) · [End-to-end checks](../spikes/engine/tests/animated_images.rs)

### Later implementation

- **Replacing Skia still decoders.** Keep each replacement behind the still-image boundary. Before switching a reader, add original CC0 fixtures for its supported behavior. A headless Metal review on Apple M4 Pro verified transparent PNG, baseline RGB `.jpg`, progressive RGB `.jpeg`, grayscale and an untagged CMYK sample. All eight JPEG EXIF orientations matched the oriented reference. A JPEG with an embedded linear RGB profile matched LittleCMS within one 8-bit level per channel. Preserve oriented dimensions for picking and painting. The CMYK sample qualifies that case only. Still WebP retains its documented coded-orientation policy. [Fixture policy](assets.md)
