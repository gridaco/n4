# Authored cameras and editorial cuts

A named camera describes a viewpoint in one picture detail. A cut selects a viewpoint for **one placement** of that picture. The project master exposes the cuts alongside the rest of the edit. Two placements can reuse the same artwork and camera animation with different cut schedules. [Master format and camera resolution](../../../engine/tests/camera.rs)

## Placement-owned cuts

From the Signal Atlas master, with times in project-rate frames:

```text
[128,192) V1 survey
  [0,12)  CAM wide
  [12,48) CAM detail
  [48,64) CAM wide
[128,192) V2 transmission
```

The survey detail declares `<camera id="wide"/>` and `<camera id="detail" center-x="608" center-y="263" zoom="1.22"/>`. At frame 140 the detail camera takes over, the survey animation reaches its local frame 12, and the V2 transmission stays in output coordinates. The earlier use of the same survey has its own framing. [Survey artwork](../../../examples/signal-atlas/sequences/survey.xml) · [Master](../../../examples/signal-atlas/project.proj)

- Declare cameras as direct children of the picture's XML sequence. IDs are unique within that detail. Two different details may use the same ID. Declaration order has no effect on painting or camera selection.
- A supplied `CAM` schedule covers its parent V1 placement exactly once, using contiguous half-open intervals in placement-local frames. Every ID resolves in that placement's detail. Gaps, overlap and unresolved IDs fail validation. A placement without a schedule uses the detail's static `camera` attribute, or identity framing when omitted.
- The shot-local clock continues across cuts. The evaluator selects the camera for the output frame and samples its animated pose on the existing shutter clock. Primary footage has its own source trim. Timeline inspection reports the selected camera and whether it came from the default or a placement cut. [Camera regressions](../../../engine/tests/camera.rs) · [Frame evaluation](../../../engine/src/plan.rs)

## Geometry

An orthographic camera uses a center in logical project pixels, a positive zoom and a signed clockwise roll in degrees. Defaults are the project center, zoom 1 and roll 0. With project dimensions `W × H`, center `c`, zoom `z` and roll `r`, point `p` appears at `(W/2,H/2) + z × R(−r) × (p−c)` in a Y-down canvas. Panning right moves the artwork left. A preview drawn at half resolution shows the same logical region.

A perspective camera adds vertical field of view, position in meters and rotation in degrees. Objects in one `<scene3d>` share its depth space. A scene filter can use resolved color and named object mattes before subsequent graphics are composited. The Field Unit project cuts between three views of one model. [Camera and scene syntax](sequence-xml.md#named-cameras) · [Field Unit master](../../../examples/field-unit/project.proj)

## Rendering and editor ownership

The camera frames the primary picture and its shot-local graphics. A camera can reveal artwork outside the initial frame. V2 remains useful for a title or station ID that should stay fixed to the screen. Editor pan and zoom belong to the viewer session. To publish a different framing, edit the camera pose or the master cut.

A per-camera look would need an explicit attachment point. A filter on a spatial scene already processes the selected view of that scene. A finish applied after V2 would also affect screen graphics. These give different results when cutting from a graded close-up to a clean wide shot beneath the same title.

SVG named views and OpenUSD cameras separate the definition of a view from the artwork it frames. Unreal's Camera Cut Track provides a precedent for making the active camera visible in the edit. [SVG views](https://www.w3.org/TR/SVG2/linking.html#ViewElement) · [OpenUSD camera](https://openusd.org/release/api/class_usd_geom_camera.html) · [Unreal Camera Cut Track](https://dev.epicgames.com/documentation/en-us/unreal-engine/cinematic-camera-cut-track-in-unreal-engine)
