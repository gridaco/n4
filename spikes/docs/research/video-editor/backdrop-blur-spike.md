# Backdrop blur through a mask

A frosted caption needs three paint positions. Draw the footage and any underlying graphics, blur those pixels through the card silhouette, then draw the caption text. A moving graphic blurs while passing behind the card and stays sharp when placed above it. The synthetic fixture includes an animated mask driven by authored coordinates and project time. [Frosted caption fixture](../../../engine/fixtures/creator/frost.json) · [Coverage shader](../../../engine/fixtures/creator/frost-mask.wgsl)

## Engineering contract

1. At the adjustment's paint position, expose the current destination as a sampled image. A spatial parent preserves the surrounding destination. Inside an isolated group the adjustment sees the children already painted there. A second adjustment sees the result of the first. Resolve the input for each shutter sample.
2. The Gaussian recipe accepts sigma from 0 to 16 project pixels. The renderer schedules horizontal and vertical passes with finite support of `ceil(3 × sigma)` on each side, and bypasses both at zero. Scrubbing, playback, inspection and export use the same full-quality WGSL kernel. A replacement kernel can read neighboring samples within its declared support. [Backdrop scheduler](../../../engine/src/render/gpu/backdrop.rs) · [Gaussian recipe](../../../engine/fixtures/creator/gaussian-blur.wgsl)
3. Let `B` be the current backdrop, `F` its filtered result, and `m` the mask's fractional coverage. Replace the affected pixels with `mix(B,F,m)` in linear premultiplied RGBA, including alpha. Retaining the original samples avoids double-counting a semitransparent backdrop.
4. Crop writes to the visible mask bounds and include the kernel halo when reading the backdrop. Clamp samples at the real scope edge. Scratch allocation and work depend on the visible area and support. The GPU keeps sampling and output surfaces separate. [Performance guide](../../../../docs/performance.md)

## Evidence

The CPU-convolution comparison checks the cropped output, fractional edges and a bright sample outside the mask influencing an interior pixel. Other controls exercise group isolation, paint order, translucent inputs, and animated mask coverage during exposure. [Pixel and order regressions](../../../engine/src/backdrop_tests.rs) · [Headless fixture checks](../../../engine/tests/backdrop.rs)

For measurement, compare bypass against the enabled effect on the same visible area, then increase the mask size at fixed sigma. Record pass count, peak scratch, and p95 hold during a scrub. Source blur has a different sampling contract and reads the drawable in source pixels. [Source blur](source-effects.md#blur-on-the-source)
