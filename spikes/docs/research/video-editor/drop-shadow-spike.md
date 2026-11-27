# Drop shadow from a layer’s alpha

A drop shadow takes its caster from the source's visible alpha after its local mask and reveal. Blur the silhouette, apply a signed offset and RGBA tint, and composite the sharp source over it in linear premultiplied RGBA at the source's paint position. An isolated group casts one shadow from the combined children. Subsequent paint can cover both caster and shadow. [Shadow compositor](../../../engine/src/render/gpu/shadow.rs)

The bundled Gaussian uses project pixels for offset and sigma. Tint alpha controls strength. Zero sigma gives a hard offset silhouette. Zero tint alpha gives the original picture. Changing the subject's RGB while keeping alpha fixed preserves the Gaussian shadow. The internal fixture field `underlay` binds the kernel, offset, sigma and tint. [Shadow controls](../../../engine/src/shadow_tests.rs) · [Gaussian recipe](../../../engine/fixtures/creator/gaussian-blur.wgsl)

## Bounds and cost

The paint bounds include the source and its shadow. Expand the offset caster bounds by the finite Gaussian support, `ceil(3 × sigma)`, on every side. Sample transparent pixels beyond the caster. A primitive just outside the stage can cast a visible shadow back into the picture. Clip work using both the output bounds and the source samples needed to produce the shadow. Clipping the source at the stage edge first drops valid contributions.

The headless pixel tests compare a masked, translucent caster to independent CPU convolution, including negative offset and an offstage caster. Additional regressions exercise the combined silhouette of an isolated group, later foreground paint, and scratch allocation based on the visible effect. For performance comparisons, report the masked area and blur support alongside the frame time. [Shadow regressions](../../../engine/src/shadow_tests.rs) · [Performance guide](../../../../docs/performance.md)
