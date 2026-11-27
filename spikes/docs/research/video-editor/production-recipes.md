# Production recipes

A recipe describes a shot in terms a creator can review. The supplied projects use original CC0 artwork, type and sound. Each trial below can be attempted using only a copy of a project and its included media. [Example catalog](../../../examples/README.md)

## An editorial map

A survey film returns to the same map after a cutaway. The viewer should recognize the coastline while the new framing draws attention to a smaller area. Keep the map plate, the route drawn across it, the station indicator and the labels independently editable. The source of geographic or survey data can remain outside the editor.

Start with **Signal Atlas**. Park at frame 42. Change the survey line's color in the shared XML definition, then check the opening, the sounding cutaway and the reprise. Both survey shots should pick up the edit. The route should stay registered to the map through the reprise camera move. Toggle V2 to isolate the station overlay. The echo profile and soundtrack should still play through. [Project](../../../examples/signal-atlas/README.md) · [Reload walkthrough](live-edit-demo.md)

## A kinetic headline

A short headline arrives in two beats. The first word settles, the second catches up, and both stay readable for a moment before the cut. Keep the copy editable. A designer should be able to slow the second entrance or change a word without rebuilding letter outlines.

Use the supplied typeface and replace a title in a working copy of Signal Atlas. Start from a supported range animator and animate its range end across two keys. Change a word, compare the start and end frames, then inspect one frame in the middle of the stagger. Try a word that wraps to the next line. Check that the actual line break, cluster order and held frame match the intended beat. This is an exercise for the author. The shipped survey uses its own title timing. [Text syntax](sequence-xml.md) · [Text range operations](../../../engine/src/graphics/text_animation.rs)

## A product reveal

A recorder opens on a wide shot. A cut brings the display close enough to read. The green trace continues moving, a guard passes in front of the display and the final shot reveals the whole instrument. Screen-space credits stay legible across the camera cuts.

In **Field Unit**, move the lid key and change the screen artwork independently. Step across frames 49–50 and 100–101. The signal should keep its own clock while the perspective and occlusion change. This trial distinguishes an editable object in the same depth space as its display from a rendered movie placed over the shot. Opening the supplied export takes only a video player. Rendering the edited stage requires the documented OpenUSD SDK. [Project and model source](../../../examples/field-unit/README.md) · [Import profile](../../../spatial/openusd/PROFILE.md)
