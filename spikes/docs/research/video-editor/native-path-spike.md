# Native path geometry and coverage

Use a native path when a creator needs to edit a contour, animate its shape or trim a stroke by distance. A path belongs to a visual sequence and can share a timeline placement with text, footage and other shapes. An imported SVG remains one external artwork source. [Editable survey path](../../../examples/signal-atlas/sequences/survey.xml) · [Sequence XML](sequence-xml.md#artwork-vocabulary)

## Authoring and paint

A path accepts SVG `d` commands in local units, nonzero or even-odd winding, optional solid RGBA fill, and a centered stroke with width, cap and join. A stroke-only path uses `fill="none"`. For animated native cubic contours, each pose provides corresponding anchors and handles. Width and trim endpoints can animate independently on the path's clock. An arbitrary path-data string cannot be interpolated with an unrelated one. [Path and pose syntax](sequence-xml.md) · [Trim semantics](trim-paths.md)

The Skia painter evaluates the contour and produces antialiased local coverage. Raster bounds carry their local origin, including negative coordinates and projecting caps. The compositor maps this source through the layer transform and passes local coordinates to a WGSL effect. A placement edit can reuse the local raster. [Path painter](../../../engine/src/render/path.rs) · [Compositor](../../../engine/src/render/gpu.rs)

## Masks

A receiver can use a local path as a mask. Its filled geometric coverage is independent of the color or outline used to paint a visible copy. A mask sourced from an SVG instead uses the artwork's painted alpha. When the receiver is an isolated group, the mask applies to the combined children once. Group translation moves both the artwork and its mask.

The headless regressions exercise holes, off-origin bounds, stroke coverage, and mask ownership. The synthetic creator fixture also combines masked geometry with a point shader. Use these controls when changing path parsing, rasterization or placement. [Path regressions](../../../engine/tests/path.rs) · [Synthetic fixture](../../../engine/fixtures/creator/path.json)
