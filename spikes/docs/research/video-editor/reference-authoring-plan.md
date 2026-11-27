# Where authoring controls belong

A creator should be able to say “move this word two frames later,” “let the ink pick up the paper beneath it,” or “use that shot twice with different framing.” Each request identifies what should stay editable when the creator changes their mind. Try the edit with the [production recipes](production-recipes.md) before proposing syntax.

## How a capability earns a primitive and an owner

1. Describe a visible or audible result and the independent edit a creator wants to make. Check whether the current contract preserves that intent after save and reopen.
2. Compare the smallest transferable concept in a predecessor. SVG gives a path geometry and paint. After Effects associates an animator and a selector with editable text. A 3D scene graph gives children a parent transform. Write down the evidence for choosing a concept and what might justify revisiting it.
3. Place the control where its inputs are available. Test it on two different kinds of artwork or two uses of the same asset. If the proposed owner has already discarded the information the edit needs, change that boundary.
4. Show the operation in authored text and through the headless evaluator. Check a nontrivial frame, the neighboring cut and a revision where only the intended control changes. Then inspect it in the viewer.

## Edit authority

| Edit | Technical concept | Owner |
| --- | --- | --- |
| Cut a shot back into the film after a cutaway | Placement referencing a reusable definition, half-open time interval | Editorial master and timeline evaluator |
| Pan the chart and its annotations together | Local coordinate system and an animatable parent transform | Sequence hierarchy and evaluator |
| Delay a headline entrance | Keyframed property and text-range selector | Sequence, timing evaluator and text provider |
| Recolor a label or change a corner | Text paint or rectangle geometry with radius | Native visual layer |
| Change ink density over a moving plate | A blend of source and backdrop in the working color space | Compositor |
| Build a synthetic map | External data converted to artwork with explicit provenance | Project author or optional tooling, registered as an asset |
| Repeat a known edit across 30 shots | Structured read and write of authored objects | SDK and project script |

Keep preparation and delivery distinct. An artist can draw an original map in another application. Once imported, placing it, masking it and revealing a route over it are ordinary authored edits. The project can use Python to generate variations. The resulting project should open on another machine with its declared media. Reference matching and review renders are production methods. They need no special primitives. [Example template](../../../examples/signal-atlas/AGENTS.md)

## Editable headline choreography

Keep the string, style and wrap width visible to the text engine. The animator chooses which text units are affected and how a sampled value is applied to those units. Changing the spelling must trigger shaping again, and a glyph cluster can contain more than one character. Preserve a meaningful mapping to the source string when a ligature crosses the edited range. An outline artist can convert lettering to paths when hand editing its contours becomes the creative task. [Text range implementation](../../../engine/src/graphics/text_animation.rs) · [Adobe text animators and selectors](https://helpx.adobe.com/after-effects/using/animating-text.html)

## How to revisit a decision

Use a small reproducible film, enter two alternatives, and inspect the controls needed to revise either one. Record what an author had to generate, what the SDK can safely manage, and what belongs in the shared evaluator. The [graphics inventory](graphics-capabilities.md) records remaining capabilities and the [ledger](ledger.md) describes the production checks.
