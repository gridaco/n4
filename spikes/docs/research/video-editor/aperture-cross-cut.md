# Animated apertures and overlays across cuts

A rectangular window over a moving picture involves three independent edits. Widening the window changes mask geometry. Panning the image changes the image transform. Moving the entire entrance earlier changes the overlay placement on its parent timeline. Keeping these controls independent allows a creator to resize the window and still retime the entrance in one master edit. [Master placement tests](../../../engine/tests/overlay_edit.rs) · [Sequence syntax](sequence-xml.md)

## A window over moving artwork

An isolated group owns its mask. Use an editable rectangle inside the mask and place the picture inside the group. The compositor combines the children before applying coverage. A held window can crop a moving picture, and an animated window can reveal a stationary picture. Painted and masking rectangles share the same evaluated contour. Signal Atlas uses a rounded mask over the relief image and survey line, so a radius edit changes a single edge shared by both. [Survey source](../../../examples/signal-atlas/sequences/survey.xml) · [Geometry evaluation](../../../engine/src/render/rect.rs)

## A placement across a cut

In a common parent, one V2 placement can span the cut between two V1 placements. Its animation follows the V2 local clock. Moving the base cut changes the visible picture beneath the overlay. Moving the V2 interval changes its entrance relative to both pictures. A lower third over an interview cut uses the same model. The master owns the lifetime and lane. The XML detail owns the overlay's layout and keys. [Timeline requests](../../../engine/src/plan.rs) · [Placement regression](../../../engine/tests/overlay_edit.rs) · [OTIO track and stack semantics](https://opentimelineio.readthedocs.io/en/latest/tutorials/otio-timeline-structure.html)

## Rectangle coordinates and resize handles

A rectangle stores local `x`, `y`, `width` and `height`. Its optional circular `corner-radius` takes one value or four values clockwise from the top left. Reduce all radii proportionally when adjacent corners exceed an edge. The layer can also translate, rotate or scale around its own pivot. [SVG rectangle geometry](https://www.w3.org/TR/SVG2/shapes.html#RectElement) · [CSS corner overlap](https://www.w3.org/TR/css-backgrounds-3/#corner-overlap) · [N4 rectangle syntax](sequence-xml.md#rectangle-corners)

A centered resize can write two properties in one editor command. Widening a rectangle from 100 to 140 at `x=30` writes `x=10` to keep its center at 80. During animation, the width and position curves must share eased progress for that center to stay still between keys. A center constraint would warrant its own authoring contract if it becomes important to preserve across independent retiming. Scaling the entire parent would also scale the cropped content.

## Regression cases

- Compare the alpha edges of painted and masking rectangles at the same fractional sample. Exercise zero size, overlapping corner radii, a translated parent and a picture moving under a held mask. [Rectangle evaluator](../../../engine/src/render/rect.rs) · [Geometry tests](../../../engine/tests/editor_geometry.rs)
- Roll the cut between two different base pictures underneath a single lower third. At the same project frame its local time and coverage should agree before and after the edit. Move only the lower third and verify its first and last visible frames. Repeat inside a parent starting at a nonzero frame. [Overlay edit regressions](../../../engine/tests/overlay_edit.rs)
