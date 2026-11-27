# Layer and group transforms

A drawable and either kind of group carry an editable local transform. A parent can reposition a diagram while a child keeps its own animation, stroke and mask. Signal Atlas places its sounding artwork under a shared parent. [Sounding sequence](../../../examples/signal-atlas/sequences/sounding.xml) · [Authoring syntax](sequence-xml.md#layer-and-group-transforms)

## Coordinate contract

For a local point, the parent-space result is `placement + translation + pivot + rotation × scale × (point − pivot)`. Translation is in parent pixels, pivot in local pixels, and positive rotation is clockwise on the Y-down canvas. The pivot defaults to the local origin. Use one positive uniform scale or independently positive X/Y scales. A signed angle is interpolated before building the matrix, so a key from 0° to 720° retains both turns. Geometry dimensions, paragraph wrap width and local stroke width keep their own units.

The evaluator samples channels on their owner's clock and combines transforms as parent matrix × child matrix. A 45° child under a parent stretched horizontally by 2 acquires shear. Preserving the full affine basis through sampling keeps the image, mask and local filter coordinates registered. The renderer can reuse local artwork across a placement-only edit. [Affine evaluator](../../../engine/src/render/affine.rs) · [Pixel regressions](../../../engine/src/render/gpu/affine_tests.rs)

## Spatial parenting and group isolation

- `<transform-group>` supplies a coordinate system. Its children paint in order onto the current destination. A backdrop inside it can sample earlier paint outside the group.
- `<g>` collects its ordered children on transparency. The compositor can mask, filter, fade or blend the resulting surface as a unit. A partly transparent group edge over two overlapping children receives the group coverage once. [Grouping regressions](../../../engine/tests/transform_group.rs)

SVG nested coordinate systems and Godot Node2D both retain local controls on children. N4 specifies one local operation order. Pivot placement, proportional resize and preserving a world pose while reparenting belong to editor commands. Reparenting sheared artwork may require a local matrix that the editable rotation and scale controls cannot express. [SVG nested transforms](https://www.w3.org/TR/SVG11/coords.html#NestedTransformations) · [Godot Node2D](https://docs.godotengine.org/en/stable/classes/class_node2d.html)
