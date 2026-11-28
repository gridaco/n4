# Engine-owned WebP import

**Still and animated WebP import implemented.** Use an authored image layer for a still WebP. The [primary raster image contract](../../docs/image-formats.md) defines playback for animated WebP and GIF. The creator chooses a still or a clip using the existing XML vocabulary. N4 owns asset identity, color, alpha, placement and time. A codec library decodes the compressed pixels.

## Where Skia participates today

N4 identifies an authored WebP from its bytes and uses its Rust decoder for intrinsic size and pixel data. GIF has a separate Rust reader. PNG and JPEG go through Skia’s `Image::from_encoded`. All still readers produce premultiplied sRGB RGBA8 for the same raster cache. Timed WebP enters through the media reader and returns straight RGBA8 with explicit coverage. wgpu handles transforms, filtering, blend modes and composition. The editor also uses intrinsic size for rectangular picking, including transparent pixels. Resizing a composed thumbnail uses Skia separately. Embedded USD textures have their own PNG/JPEG decoder. [Still reader](../engine/src/render/image.rs) · [Paint cache and prepared assets](../engine/src/graphics.rs) · [GPU color conversion](../engine/src/render/gpu/linear_premul.wgsl) · [Thumbnails](../engine/src/preview.rs) · [USD reader](../spatial/openusd/src/lib.rs)

## Decoder choice

Use **`image-webp` directly for stills**. Version 0.2.4 offers lossy and lossless decoding in Rust, intrinsic dimensions, animation detection, ICC bytes and sequential access to composited animation frames. The broader `image` crate wraps the same decoder and is useful for projects needing a multi-format API. The existing still reader stays independent of the animated backend. For animation the engine uses libwebpdemux’s straight RGBA reader to preserve uncovered pixels even when every encoded patch is opaque. Use libwebp as an independent reference when qualifying the Rust decoder. Enabling Skia’s WebP feature would leave asset interpretation attached to the painter and still require timeline integration. [Rust decoder](https://docs.rs/image-webp/latest/image_webp/struct.WebPDecoder.html) · [image wrapper](https://docs.rs/image/latest/image/codecs/webp/struct.WebPDecoder.html) · [libwebp API](https://developers.google.com/speed/webp/docs/api) · [libwebp animation API](https://github.com/webmproject/libwebp/blob/main/src/webp/demux.h)

## Authored stills

```xml
<image name="Decal" source="assets/decal.webp" position="96 48"/>
```

1. **Asset recognition.** Both probing and decoding inspect the RIFF/WEBP signature, including when the file has been renamed. A recognized but invalid WebP produces a path-specific error. Animation is rejected during import, before a still reader can return frame zero. PNG and JPEG can migrate through this same interface after parity tests cover their accepted profiles and pixels.
2. **Color and alpha.** The reader accepts VP8, VP8L and extended stills with alpha. Picking and paint use the coded canvas and ignore EXIF rotation and mirroring. Untagged input uses sRGB. For a supported embedded RGB ICC profile, `moxcms` converts straight RGB to sRGB using relative colorimetric intent. N4 preserves coverage separately, expands opaque RGB to RGBA and premultiplies once in encoded sRGB. Malformed or unsupported profiles produce errors. [Pixel conversion](../engine/src/media/webp.rs) · [WebP color profile](https://datatracker.ietf.org/doc/html/rfc9649#section-2.7.1.4)
3. **Allocation and asset lifetime.** A WebP file may occupy at most 256 MiB, with an embedded ICC profile up to 4 MiB and canvas axes up to 8,192 pixels. These are per-asset limits. A 4K RGBA still needs about 32 MiB once decoded. The engine keeps an immutable decoded raster for reuse. Editor snapshots share that raster with picking, playback and the thumbnail workers. A new revision validates a replacement before installing it. A readable header with broken pixel data leaves the previously accepted artwork available even to a cold thumbnail request. [Snapshot preparation](../engine/src/host/mod.rs) · [Input observations](../engine/src/project_watch.rs)

**Regression coverage.** The original [CC0 controls](../engine/fixtures/webp) carry known color patches and independent decoded witnesses. [CPU tests](../engine/src/render/image/tests.rs) · [Fixture policy](../../docs/assets.md)

| Check | Evidence required |
| --- | --- |
| CPU decode | Lossless RGBA matches the authored values, including zero and fractional alpha. Check opaque lossy and lossy-with-alpha against libwebp, using a stated RGB tolerance for upsampling and exact alpha. An untagged still, a non-sRGB RGB profile and an invalid profile exercise the color policy. |
| Failure handling | A renamed valid file opens by signature. Animated, oversized, truncated and corrupt inputs return contextual errors. Include a truncated payload whose dimensions can still be read. |
| Normal production use | Place lossless WebP and equivalent PNG over black and white, transform them and apply a source effect. Compare full composed pixels in headless renders and encoder input, plus a graphics-only shot thumbnail. Seek backward and verify the warm raster is reused. Check editor bounds after replacing the file with different dimensions and check a failed replacement followed by a repair. [Image motion](../engine/tests/image_motion.rs) · [Editor geometry](../engine/tests/editor_geometry.rs) · [Filmstrip](../engine/tests/proxy.rs) |

**Qualification.** On Apple M4 Pro / Metal, a 960×540 project used Field Unit’s public WebP cover as its sole image. The headless CLI validated it, rendered frames 2 then 0, produced a thumbnail and exported a three-frame H.264 MP4. Both full-size renders matched the libwebp reference at every RGB byte. The moving-decal regression separately checks the hash of the pixels delivered to the encoder. [Field Unit cover](../examples/field-unit/preview.webp)

## Original decoder qualification

Before implementation, the debug CLI on Apple M4 Pro / Metal rendered frames 0 and 15 of a generated PNG control. Both its still WebP and animated WebP counterparts failed to decode as image layers at frame 0. An isolated program built offline with Rust 1.98.1 and `image-webp` 0.2.4 decoded a 32×24 lossy RGB still, recovered exact straight RGBA and ICC bytes from a lossless still, and read two distinct animation frames held for 80 and 120 ms. Resetting the decoder reproduced the first frame byte for byte. This probe established decoder fit. The still-import tests above cover the integrated engine. The [timed import qualification](../../docs/image-formats.md#decoder-ownership-and-qualification) covers the integrated animation reader.
