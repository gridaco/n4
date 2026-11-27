# Stroke trim by distance

A painted native path has two independently animatable scalar properties, `stroke-start` and `stroke-end`. They select the interval painted along its centerline. Redrawing a bend preserves the timing curves. Retiming the trailing tip preserves the path geometry and the leading tip. In Signal Atlas, the survey line reaches its full length at local frame 46. Change that key to adjust both occurrences in the master. [Survey sequence](../../../examples/signal-atlas/sequences/survey.xml) · [Sequence XML](sequence-xml.md#stroke-start-and-end)

## Evaluation

| Case | Behavior |
| --- | --- |
| Units and direction | Endpoints are finite fractions in `[0,1]`, defaulting to 0 and 1. Distance follows the local centerline in drawing order, before layer or parent transforms. |
| Paint | Preserve the original fill. Stroke the selected interval with the sampled width, joins and caps. Equal or crossed endpoints yield empty stroke coverage. |
| Multiple contours | Accumulate drawn length in authoring order. A move-to contributes zero length. The next contour starts without a painted bridge. Separate paths can have independent clocks. |
| Closed contours | Include the closing segment in the measured length. Fully included closed contours retain their closing join. Partial contours end in open caps. A zero-length contour consumes no distance. |
| Animation | At each requested frame or shutter sample, evaluate the path shape, measure it, sample both endpoints and apply the current width. Linear keys on a stationary curve produce constant local travel per frame. |

Shapes use existing scalar tracks, including hold and cubic easing. Width remains independently animatable. A mask can instead reveal layers by moving an edge across the picture, spending time crossing any blank space between contours. [Path renderer](../../../engine/src/render/path.rs) · [Path regressions](../../../engine/tests/path.rs)

## Decision to revisit at cutover

Preserve the independent head, tail and geometry edits across a change of renderer or syntax. Core Animation's shape-layer endpoints offer a direct precedent. After Effects and Lottie extend this operation across multiple paths and expose an offset. A separate geometry operation would become useful when another tool needs the extracted curve or its moving tip. [Core Animation strokeStart](https://developer.apple.com/documentation/quartzcore/cashapelayer/strokestart) · [Lottie Trim Path](https://lottie.github.io/lottie-spec/latest/specs/shapes/#trim-path)

SVG dash length and phase can express a single-contour draw-on. Dashing restarts on each subpath, and an authored dash pattern has its own lengths and offsets. An interchange implementation should test a filled compound shape with disconnected strokes, a closed join, a changing width and independently moving endpoints. [SVG dashing](https://www.w3.org/TR/SVG2/painting.html#StrokeDashing) · [SVG distance along a path](https://www.w3.org/TR/SVG2/paths.html#DistanceAlongAPath)
