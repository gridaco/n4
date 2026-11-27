# Editing a film with N4

Open a project, park on a frame, change one word, then come back to the same frame. The layout and timing you can inspect there should be the ones you deliver. This brief describes the creator experience. For supported commands and syntax start with the [authoring guide](../../../../llms.txt).

## Start with a finished edit

Copy Signal Atlas into your own project directory. It comes with its editable master, artwork, type, sound and a rendered film to compare against. The master shows the order of the shots, which tracks overlap and where the camera cuts. Open a sequence XML file to inspect its artwork. Edit the title or the orange route while the viewer is parked on the survey. The first and last shots use that definition. [Template and commands](../../../examples/signal-atlas/README.md)

## Give the work to another editor

An author with a text editor can adjust the key that ends the route reveal. A program can read or generate the same authored XML. The creator can inspect the evaluated layers and their timing and select an object at a frame. Changing a track's visibility in the editor can be undone, saved and inspected after reopening. The project's own guide tells a collaborator which files carry intent and which ones are generated artwork. [Editor session](../../../rfcs/0008-editor-session.md) · [Project instructions](../../../examples/signal-atlas/AGENTS.md)

## Keep visual decisions editable

A chart can have a shared plate, a moving path in its own paint and an independent label. A filter can act on a source, or a compositor can sample what is already drawn below a masked card. The source, backdrop and composition time must be explicit for a result to survive a direct seek. A 3D shot can incorporate imported geometry and put a live graphic on its surface. A handwritten effect or generated asset should leave the editing decisions that belong to the creator visible. [Render contract](rendering-pipeline.md) · [Editable product shot](../../../examples/field-unit/README.md)

## What a release must demonstrate

A solo creator should be able to replace media, revise a title after approval, locate a change at a cut, reopen the project and export the reviewed movie with sound. Broader editing, extension and delivery promises each require reproducible evidence. [Release expectations](creator-expectations.md) · [Public regression guide](../../../../docs/tests.md)
