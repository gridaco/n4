# Graphics capabilities

A supported drawing operation should have a repeatable edit and an observable result. The table links the alpha's current behavior to its source, tests and two editable CC0 examples. Signal Atlas covers flat artwork, masks and timing. Field Unit covers imported geometry, lighting and live material artwork. The [production vocabulary ledger](ledger.md) describes the results in creators' terms. [Signal Atlas](../../../examples/signal-atlas/) · [Field Unit](../../../examples/field-unit/) · [Performance guide](../../../../docs/performance.md)

## Supported in the spike

| Capability | What the creator controls | Evidence |
| --- | --- | --- |
| Native paths | SVG path commands, or corresponding cubic anchors and handles across animated poses. Filled contours support nonzero and even-odd winding. | [Path renderer and tests](../../../engine/src/render/path.rs) |
| Rectangles and masks | Local position, dimensions and one or four corner radii. The same evaluated shape can be painted or mask a group's finished picture. | [Rectangle geometry](../../../engine/src/render/rect.rs) · [Survey sequence](../../../examples/signal-atlas/sequences/survey.xml) |
| Fill, stroke and draw-on | Independent solid paints, fractional stroke width, cap and join. A path's leading and trailing tips follow normalized centerline distance while the original fill is retained. | [Trim decision](trim-paths.md#decision-to-revisit-at-cutover) · [Path edit test](../../../engine/tests/path.rs) |
| Imported SVG | One artwork layer with path fills and strokes, solid color or user-space linear gradients, ordered stops, stop opacity and group opacity. | [Gradient illustration](../../../engine/fixtures/creator/stereo-field.svg) |
| Native text | Editable copy, font, paragraph width, wrapped lines and a fitted backplate. Reveal by grapheme count, address a named span, animate a range or attach a sibling to measured text. | [XML vocabulary](sequence-xml.md#artwork-vocabulary) · [Layout](../../../engine/src/graphics/text_layout.rs) · [Range animation](../../../engine/src/graphics/text_animation.rs) |
| Image and video sources | Place an alpha image or an independently trimmed video layer. A primary shot can use footage or a solid background with optional seeded grain. | [Image and seek regressions](../../../engine/tests/image_motion.rs) · [Monitor footage](../../../examples/field-unit/sequences/instrument.xml) |
| Local transforms and keyframes | Pan, rotate around a pivot or scale on independent axes. Children inherit the parent's full affine basis. Scalar channels have independent hold, linear or cubic timing. | [Transform regressions](../../../engine/tests/transform_group.rs) · [Frame and exposure tests](../../../engine/tests/property_channels.rs) |
| Isolation, masks and knockout | Spatial parents preserve destination access. Isolated groups combine children before group opacity, mask, or blend. Destination-out removes previous coverage. | [Group transforms](layer-group-transforms.md) · [Knockout regressions](../../../engine/src/render/gpu/knockout_tests.rs) |
| Layer blending and source effects | Multiply over current paint. Blur a drawable in source pixels, animate a focus band or apply an ordered RGB grade. | [Processing contract and tests](source-effects.md) |
| Point, neighborhood and temporal shaders | Attach local ink to a stroke, sample a bounded backdrop around a lens, or access a bounded history of the primary footage for a phosphor trace. Each case has distinct inputs. | [Point shader fixture](../../../engine/fixtures/creator/path.json) · [Backdrop sampling](../../../engine/src/render/gpu/backdrop.rs) · [History sampler](../../../engine/src/render/gpu/history.rs) |
| Shadow and frosted glass | Cast a shadow from masked source coverage or blur earlier paint through a card while later text stays sharp. | [Shadow regressions](../../../engine/src/shadow_tests.rs) · [Backdrop regressions](../../../engine/src/backdrop_tests.rs) |
| Cameras and cuts | Name views in the artwork, key framing and select a view per picture placement. An output overlay can hold its position while the picture cuts. | [Camera and thumbnail tests](../../../engine/tests/camera.rs) |
| Spatial materials and lighting | Animate a rigid part at its imported pivot, bind editable artwork to a named UV slot, rotate the environment, move a key and adjust exposure. | [Field Unit](../../../examples/field-unit/sequences/instrument.xml) · [Lighting tests](../../../spatial/wgpu/tests/studio.rs) |
| Shared depth and object mattes | Place native text planes in the same depth space as rigid USD objects. A scene shader can use projected, visible or named-occluder coverage. | [Spatial regressions](../../../engine/src/render/gpu/spatial/tests.rs) · [Matte syntax](sequence-xml.md#object-mattes-in-a-scene-filter) |

## Proposed capabilities and their proofs

| Candidate | Edit that would justify adding it |
| --- | --- |
| Per-camera look | Grade only a close-up, then cut back to a clean wide view under an unchanged screen title. Specify whether the look applies before screen graphics. |
| Gradient and pattern paint | Reuse a two-stop gradient on native fill and stroke, change an endpoint, then resize the object. For tiled patterns and radial gradients, specify how paint coordinates respond. |
| Stroke styling | Animate dash phase around a turn. Test multiple subpaths, non-scaling width, miter limits and stroke-before-fill as independent requests. |
| Geometry-aware brushes | Taper a dry-brush stroke through an S-curve and keep its bristles aligned to the curve direction while editing a handle. |
| Path booleans | Subtract one editable path from another and stroke the newly cut boundary. A raster knockout only supplies the composited pixels. |
| Shaped text access | Animate a chosen shaped word while neighboring runs retain layout. Exercise a ligature, a combining mark and a changed line break. |

When adding a row, name the editable input, its owner and a headless way to observe the result. An imported asset rendering a feature proves asset support. To claim native authoring, edit that feature in the source sequence and render again.
