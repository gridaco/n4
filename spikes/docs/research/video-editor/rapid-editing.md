# Rapid editing and virtual assets

Authors cut, slip, replace, duplicate and temporarily disable parts of a timeline while comparing variations. A project must distinguish an asset, a reusable visual definition and each occurrence of that definition. Resolve those identities before deciding which writes or cache entries an edit affects. [Current authoring contract](../../../../docs/project-format.md)

## A virtual cut on a shared survey

In Signal Atlas, the opening and reprise refer to one survey definition. Editing its plate changes both uses. Editing the reprise camera cut affects that occurrence alone. The inactive interval between the two occurrences matters to the creator, the renderer's cache and the timeline highlights. [Master](../../../examples/signal-atlas/project.proj) · [Damage walkthrough](live-edit-demo.md)

For a proposed split command, split the placement interval at frame F, preserve the mapping into source time on both sides and retain the registered source. A split is reversible without copying media. When the creator wants one section to diverge, distinguish a second occurrence from a new definition that can be edited independently. Commands should make clear which operation was requested.

## Tagged markers and splitting at cues

A timeline marker identifies a time or span and can carry a label such as “music entrance.” A tag can collect several cues without giving the tag its own timing. Distinguish a cue tied to project time from one attached to source material before implementing ripple or slip edits. A cut at a cue should preserve the sound and picture on either side unless the author also requests a trim. These are design cases for future commands.

## Attaching artwork to a moving point

A graphic attached to the end of an animated line needs a point and perhaps a tangent on evaluated geometry. A label can follow the point while retaining its size in screen space. An attachment has different dependencies from an editorial time marker. SVG markers are a useful precedent for orienting artwork along a path. [SVG markers](https://www.w3.org/TR/SVG2/painting.html#Markers)

## File discipline for people, agents and the GUI

Use frames relative to the enclosing timeline, a small master that reveals all placements, and separate named detail files for artwork. Refer to shared sources through the registry. Keep each generated asset and the input it came from identifiable. Direct edits should preserve human-readable comments or report a conflict at save. The editor's current track-toggle transaction exercises undo, external write detection and reopen. Other operations warrant their own round-trip checks. [Editor session](../../../rfcs/0008-editor-session.md) · [Regression suite](../../../../docs/tests.md)
