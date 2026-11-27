# Two authoring paths for mixed 2D and 3D

A product shot can combine an articulated object, text attached to its surface, a picture playing on the object and credits fixed to the frame. The choice of master affects which of those edits an author can express directly. It is independent of which GPU renderer draws the result.

## A. Author the composition in USD and give it 2D semantics

USD supplies composable scene descriptions, prim paths, references, transforms, materials, cameras and time-sampled attributes. Custom schemas could represent text boxes, typography selectors and editorial intervals. Other USD consumers would need implementations of those schema semantics to reproduce the same picture. Exported meshes or textures can communicate an appearance at the cost of editable text and layout. [OpenUSD documentation](https://openusd.org/release/index.html)

## B. Author the composition in N4 XML and reference spatial content

The N4 master schedules placements. A visual sequence can introduce a spatial scene with an imported stage, native planes and a camera. Imported geometry participates in shared depth and can supply object coverage to downstream effects. Screen-space titles can be placed above that scene. The tested subset and known omissions belong in the importer profile. [Visual grammar](sequence-xml.md) · [OpenUSD profile](../../../spatial/openusd/PROFILE.md)

## Reproducible exercise

Open Field Unit. Change the hinge angle, change the typography on the serial plate, then slide a camera cut by one frame. For each change, inspect the file that owns it and the elements that stay fixed on screen or on the object. The Blender scene construction and resulting USD are supplied with the project. This tests path B. A proposal to adopt path A should make the same three edits in a text-authored stage and demonstrate which tools can edit and render the new schema. [Field Unit](../../../examples/field-unit/README.md) · [Spatial rendering proposal](../../../rfcs/0006-spatial-rendering.md)
