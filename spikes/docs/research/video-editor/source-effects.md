# Layer blending, source blur and RGB grading

Choose the owner by identifying which pixels the operation reads. A blend combines a painted layer with the destination at its position in paint order. A source effect takes its owner's input and passes the result to the next source effect. A backdrop adjustment samples the picture already painted beneath it. Moving the same operation between these positions can change neighboring layers. [XML examples](sequence-xml.md#artwork-vocabulary) · [Backdrop contract](backdrop-blur-spike.md)

## Multiply on a layer

Use `blend="multiply"` to make colored ink respond to the surface beneath it. Moving the ink across a changing picture recomputes the overlap. A child of a spatial parent sees the surrounding destination. An isolated group starts on transparency and can itself blend as one layer.

The compositor resolves in linear light after source coverage. For premultiplied source and destination RGB `s,d` with alphas `a,b`, the result is `s*d + (1−b)*s + (1−a)*d` with alpha `a + (1−a)*b`. A destination copy covers the affected region. An overlapping mark sees previous marks. The regression exercises transparent destinations, partial coverage, paint order and shadow interaction. [Blend shader](../../../engine/src/render/gpu/multiply.wgsl) · [Pixel tests](../../../engine/tests/multiply.rs) · [W3C compositing](https://www.w3.org/TR/compositing-1/#blendingmultiply)

## Blur on the source

Place `<source-blur>` on a drawable to soften that source and keep subsequent paint sharp. Sigma is 0–32 source pixels. Two directional Gaussian passes read linear premultiplied RGBA within `ceil(3 × sigma)` pixels and extend edge texels at the source bounds. The filtered image retains the source extent. An optional focus band mixes the original and blurred pictures by local Y position, sharp half-width, feather and amount. A layer mask subsequently controls visible coverage.

A held source can reuse its filtered result when the focus band or placement moves. Group-local source blur would require a defined group sampling space. An effect with transparent falloff outside the source would also require an expanded output extent. [Source blur tests](../../../engine/tests/source_blur.rs) · [Gaussian kernel](../../../engine/src/render/gpu/gaussian.wgsl) · [SVG Gaussian blur](https://www.w3.org/TR/filter-effects-1/#feGaussianBlurElement)

## Grading the print

An RGB grade applies independent gain and offset to straight, encoded sRGB. Each channel becomes `clamp(gain × channel + offset, 0, 1)`. Alpha is preserved and zero-alpha output is transparent black. This lets a creator warm a scanned image while ink painted above it keeps its authored color. A grade on an isolated group processes its composited children.

Author effects in document order. A grade before blur changes which values contribute to the neighborhood average. A grade after blur changes the averaged result. Effects may repeat. Gain and offset each accept a constant or an animated RGB triple. The grade lives on this placement's effect stack. Asset interpretation and display/export color management have separate responsibilities. [Grade and ordering tests](../../../engine/tests/rgb_grade.rs) · [Effect execution](../../../engine/src/render/gpu/source_effects.rs) · [SVG component transfer](https://www.w3.org/TR/filter-effects-1/#feComponentTransferElement)
