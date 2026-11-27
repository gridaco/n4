# N4 visual sequence XML

A picture or graphics placement in `project.proj` names a detail under `sequences/<id>.xml`. The master owns the half-open placement interval and track. A `<sequence>` owns the artwork, its paint order and its local animation. The CLI, player and exporter all decode the XML into the native graphics model. Imported SVG stays a separate asset format. Signal Atlas supplies a complete editable example. [Master](../../../examples/signal-atlas/project.proj) · [Native reader](../../../engine/src/project/xml.rs)

## Read a frame

In Signal Atlas each placement gets its own local clock. At local frame 0 the transect is hidden. By frame 46 the whole stroke is present. Both placements reference this same path, so changing its end key affects both. A rounded rectangle attached to the parent masks the complete survey group, including the synthetic relief beneath the path. [Survey source](../../../examples/signal-atlas/sequences/survey.xml)

```xml
<path name="Survey transect" d="M-12 276 C92 274 121 106 218 127 S325 271 408 196 S502 100 588 124"
      fill="none" stroke="#ff8245" stroke-width="3.5" stroke-linecap="round">
  <animate attributeName="stroke-end">
    <key frame="0" value="0" ease="cubic(0.25 0.05 0.45 1)"/>
    <key frame="46" value="1"/>
  </animate>
</path>
```

- A shot with a solid background uses `<sequence name="…" background="#ffffff">`. A shot backed by recorded footage uses `<sequence name="…" source="media/camera.mp4" source-in="12">`. The optional trim is in project-rate frames and defaults to zero. Both can contain shot-local graphics. A graphics detail on V2 uses `<sequence>` and contributes its ordered graphics over the base picture. The A1 soundtrack keeps its sample-based JSON sidecar. A placement ID has exactly one source file.
- An independent moving layer uses `<video name="Monitor" source="assets/rig.mp4" start="38" source-in="7" duration="82" size="640 360"/>`. At frame 38 of its containing sequence placement it opens on source tick 7, then advances for 82 project-rate frames. The media index maps those ticks to presentation timestamps. The decode size preserves the source aspect ratio. Frame the result using placement, transform and mask. Each layer can have its own source and trim. [Field Unit monitor](../../../examples/field-unit/sequences/instrument.xml)
- `position="x y"` places a layer in its parent. `span="start end"` makes it visible over a half-open local interval and starts its own clock at zero. Within a rectangle, `x` and `y` describe the local geometry. `translate-x` and `translate-y` animate the layer’s transformation.
- An `<animate>` element targets one property of its immediate owner. Write either a static dimension or one animation for that dimension. Scalar keys have strictly increasing integer frame numbers. Values hold before the first and after the last key. The outgoing interval defaults to linear and also accepts `ease="hold"` or `ease="cubic(x1 y1 x2 y2)"`. The native evaluator samples between frames for exposure.
- `<motion>` retains grouped pose keys with frame, x, y, scale and opacity. Separate layer channels use `<animate>` for translate-x, translate-y, rotation, scale, scale-x, scale-y and opacity. Choose one representation of the layer motion. A layer can still carry independent animations on its underlying geometry.

## Named cameras

A V1 detail may set `<sequence camera="wide">` and declare `<camera id="wide" .../>` as a direct child. The ID belongs to that detail. Paint order is independent of declaration order. A `CAM` row under a V1 placement selects an ID for its half-open interval in picture-local frames. A cut schedule covers the entire placement. With no cut rows the detail's default applies, falling back to identity framing. V2 artwork remains in output coordinates.

Orthographic cameras support `center-x`, `center-y`, `zoom` and `roll`. Perspective cameras take `projection="perspective"`, a vertical `fov-y` in degrees, and optional `position="x y z"` in meters and `rotation="x y z"` in degrees. To animate a pose axis, use the individual scalar properties (`position-x`, `rotation-y`, etc.) in place of the corresponding packed vector. Each property takes the existing `<animate>` and `<key>` syntax on the picture clock. A `<scene3d pixels-per-meter="100">` draws a referenced USD mesh and editable text planes in that camera's depth space. Its layer filter processes resolved color before V2 is drawn. [Camera semantics](camera.md) · [Perspective camera cuts](../../../examples/field-unit/project.proj) · [Orthographic cameras](../../../examples/signal-atlas/sequences/survey.xml)

### Rigid models and editable material artwork

A USD placement can rotate named rigid descendants about their imported local axes and pivots. It can also supply live N4 artwork to a named material. Bindings are local to the placement, so two instances of the same asset can show different displays. Use the absolute USD prim path for a part and the absolute bound material path for a slot. The importer reports missing paths and unusable UVs when preparing the project.

```xml
<scene3d pixels-per-meter="100">
  <studio environment-rotation="20" environment-intensity="0.8" exposure="0"
          shadow-center="0 0 0" shadow-extent="0.30" shadow-bias="0.00015"/>
  <distant-light direction="0.46 -1 -0.35" intensity="2.5" color="#fff2d3"/>
  <usd source="assets/field.usdc" prim="/World/Recorder">
    <part prim="/World/Recorder/Hinge">
      <animate attributeName="rotation-x">
        <key frame="0" value="69"/><key frame="47" value="-10"/>
      </animate>
    </part>
    <surface slot="/World/_materials/Display" role="unlit" size="640 360">
      <video source="assets/signal.mp4" start="0" duration="144" source-in="0" size="640 360"/>
      <text position="26 7" font="assets/N4GridSans.ttf" size="42" width="505" fill="#f1eacb">
        <copy>FIELD UNIT</copy>
      </text>
    </surface>
  </usd>
</scene3d>
```

The surface has a transparent pixel canvas of its declared size. Its children use ordinary 2D layer ordering, text layout and authored clocks. The mesh UVs place that canvas on the model. The `unlit` role keeps the artwork independent of the key light. The `base-color` role lets the imported material's roughness and lighting shade the artwork as print. Both participate in scene visibility. The surface accepts independent video sources. Referencing the enclosing 3D scene or its primary footage inside the surface is rejected.

Studio environment rotation is in degrees, intensity is linear and exposure is in stops. Each accepts the existing scalar `<animate>` syntax. The first distant light casts the directional shadow. The shadow center and half-extent are in scene meters, bias in meters and softness in shadow-map texels. The studio render resolves to SDR before the following 2D layers. [Tested import profile](../../../spatial/openusd/PROFILE.md)

### Object mattes in a scene filter

Give a USD placement or a text plane an `id` to let the scene's WGSL filter request its projected coverage. An ID belongs to the enclosing `<scene3d>`. An instance can refer to the same asset as another instance and have a different ID.

```xml
<scene3d pixels-per-meter="100">
  <usd id="ring" source="assets/ring.usda" prim="/Ring"/>
  <plane id="title" position="-2 1 0">
    <text font="assets/N4GridSans.ttf" size="96" width="600" fill="#221f20">
      <copy>DEPTH</copy>
    </text>
  </plane>
  <filter source="effects/ink.wgsl">
    <matte name="word" object="title"/>
    <matte name="crossing" object="title" occluded-by="ring"/>
    <matte name="face" object="ring" coverage="visible"/>
  </filter>
</scene3d>
```

The WGSL author can call `matte_word(pixel)` to sample the full projected glyph coverage, including parts hidden behind other objects. `matte_crossing(pixel)` returns the coverage where the title projects and the named ring wins the scene's visible depth test. A third object in front removes its share from that crossing. Both helpers take a scene raster coordinate (`vec2<i32>`) and return antialiased coverage from 0 to 1. Samples outside the scene return zero. The filter can sample neighboring scene pixels to trace the inside edge of a word, then limit its ink to the crossing.

`coverage="projected"` is the default for a full projection or a projection limited by `occluded-by`. With `coverage="visible"`, `matte_face(pixel)` returns just the share of the sample owned by the named object in the finished scene. Visible coverage and a named occluder are mutually exclusive.

A filter may declare up to eight mattes. Each name starts with an ASCII letter and accepts ASCII letters, digits and single underscores thereafter. Names are unique within the filter. Object IDs accept ASCII letters, digits, hyphens and underscores. Every reference resolves inside the same scene. [Native authoring types](../../../engine/src/graphics/spatial.rs)

## Layer and group transforms

`pivot="x y"` fixes the center of scale and rotation in the layer’s own pixels, with a default of `0 0`. For a point on the artwork, first scale its displacement from the pivot along the local axes, then rotate that displacement and add the pivot back. Finally apply the layer’s position and animated translation in parent coordinates. Positive rotation is clockwise on the Y-down canvas. A parent applies its transform to the entire result, including any child masks and strokes. A spatial parent keeps the children’s normal paint order and access to the destination. An isolated group composites them together.

`scale="0.84"` gives both axes the same positive multiplier. To size them independently use `scale-x` and `scale-y`, each defaulting to 1. Each scale property and `rotation` can be constant or keyed with the usual scalar easing. Give each property a single definition. Uniform scale and per-axis scale are mutually exclusive across constants and curves. The fixed X scale below works alongside an animated Y scale. A title drawn under this transform keeps its local text layout and wrap width. A rotation from 0° to 720° traces two full turns.

```xml
<transform-group name="Chart plate" pivot="36 24" scale-x="0.8" rotation="-15">
  <animate attributeName="scale-y">
    <key frame="0" value="0.5" />
    <key frame="12" value="1" />
  </animate>
  <image source="assets/relief.png" />
</transform-group>
```

The Python SDK exposes the fixed pivot and values in the layer’s `transform` mapping (`pivot`, `rotation_degrees`, `scale`, `scale_x`, `scale_y`). Its `channels` mapping holds the independent scalar tracks under `x`, `y`, `rotation`, `scale`, `scale_x`, `scale_y` or `opacity`. [Transform semantics and tests](layer-group-transforms.md)

## Rectangle corners

`corner-radius` is a length in local pixels. Its default is zero. One nonnegative value rounds all four corners. Four values name the top-left, top-right, bottom-right and bottom-left corners in that order. An animation key can also carry one or four values. The evaluator expands the value to four components and interpolates each using the key's timing.

After evaluating width, height and radii, reduce all four radii by the same factor if adjacent corners would overlap along any edge. The authored values survive resizing. With `width="120" height="40" corner-radius="100"`, each visible end has a 20 px radius. A zero width or height yields empty coverage. Painted and masking rectangles share this geometry. [Native geometry and tests](../../../engine/src/render/rect.rs)

## Stroke width

On a painted `<rect>` or `<path>`, `stroke-width` measures the thickness of the centered outline in local pixels. Use a nonnegative number, including fractions, for a constant width. To change it over time, give the shape a stroke color and author a single scalar track. Zero hides the stroke while the fill keeps drawing. Geometry, cap and join keep their own values. In this example the stroke width changes independently of the rectangle dimensions. [Path and stroke tests](../../../engine/tests/path.rs)

```xml
<rect width="320" height="180" fill="none" stroke="#fdf000">
  <animate attributeName="stroke-width">
    <key frame="0" value="5" ease="hold" />
    <key frame="2" value="13" />
    <key frame="10" value="40" />
  </animate>
</rect>
```

## Stroke start and end

A painted native `<path>` accepts `stroke-start` and `stroke-end` as constant attributes or scalar animation tracks. Both are fractions of local centerline length in drawing order, with defaults of 0 and 1. For example, `stroke-start="0.2" stroke-end="0.6"` shows the ink between 20% and 60% and preserves the original fill. Each endpoint can be keyed separately using `<animate attributeName="stroke-end">` or its start counterpart. Equal or crossed endpoints hide the stroke. On a compound path, a move-to adds no distance and paints no bridge. These names are specific to N4 sequence XML. [Decision and cutover criteria](trim-paths.md#decision-to-revisit-at-cutover)

## Artwork vocabulary

`blend="multiply"` on a painted layer combines its color with the picture underneath in linear light. Its mask, reveal and opacity control the strength. A stroke inside a `<transform-group>` can blend with the print below the group. Inside a `<g>` it sees earlier children of that isolated group. Set the blend on the `<g>` itself to multiply the finished group into its parent.

```xml
<path name="Highlighter" d="M20 64 L320 64" fill="none"
      stroke="#f0cf03" stroke-width="28" blend="multiply" />
```

### Blur one source

Place `<source-blur>` inside the drawable that supplies the pixels. `sigma` is the Gaussian standard deviation in source pixels (0–32). `amount` mixes the sharp and blurred source (0–1, default 1). With no band, the mix applies throughout the source. A horizontal `<focus-band>` keeps its center sharp. `half-width` sets the sharp core on either side of `center-y`, and `feather` sets the distance over which blur coverage rises to full. Each control accepts a constant or a scalar animation on the layer's local clock. The source stays editable under its camera transform.

```xml
<image source="assets/relief.png">
  <source-blur sigma="8">
    <focus-band half-width="24" feather="60">
      <animate attributeName="center-y">
        <key frame="0" value="80" />
        <key frame="32" value="220" />
      </animate>
    </focus-band>
  </source-blur>
</image>
```

Blur samples the drawable's finite raster, repeating its edge pixels. Its result stays inside that raster. The effect runs before the layer mask, opacity and final blend. It can be applied to a drawable under a transformed parent. Both group containers and backdrop adjustments have separate contracts. To blur the primary shot video, put `<source-blur>` inside an explicit `<footage>` layer of the sequence.

### Grade and order source effects

Place `<rgb-grade>` on the image, footage or other drawable that supplies the source. Read effect children in document order. The first receives that layer’s pixels and each subsequent effect receives the preceding result. Moving the grade above a blur changes which colors the blur averages. Effects may repeat. Sibling layers keep their own colors, even when they place the same asset.

```xml
<image name="Survey print" source="assets/relief.png">
  <rgb-grade gain="1.05 1 0.92" offset="0 0 0.02" />
</image>
```

`gain` defaults to `1 1 1` and `offset` to `0 0 0`. Values follow RGB order. For each channel, the effect computes `clamp(gain × channel + offset, 0, 1)` in straight, encoded sRGB. An offset of `1/255` adds one 8-bit code value before clipping. Alpha stays the same. Use ordinary `<animate attributeName="gain">` or `<animate attributeName="offset">` with a three-number `value` per key to change the treatment on the layer’s local clock. A grade on an isolated `<g>` receives the composited children. A `<transform-group>` only positions its children, so put a grade on a drawable inside it. [Effect order and processing space](source-effects.md#grading-the-print)

| Element | Editing semantics |
| --- | --- |
| `<g>` and `<transform-group>` | An isolated group composites its children before its mask or group opacity. A transform group moves its children together while they paint in order into the current destination. |
| `<image>` and `<svg>` | A project-relative `source` identifies an imported image or external SVG asset. |
| `<path>` | Use SVG path data in `d`, with fill, fill-rule and stroke controls. For an animated native cubic contour, use `<animate attributeName="d" closed="true">` with ordered coordinate pairs on each key. Matching point counts preserve correspondence between poses. |
| `<rect>` and `<mask>` | A rectangle may draw fill and stroke. A mask holds one supported coverage source (rectangle, path or SVG) for the finished receiver. |
| `<text>` and `<copy>` | Font, size, width and fill describe native paragraph layout. Literal copy lives inside `<copy>`. Spaces and line breaks there are part of the authored string. An identified `<span>` makes a text range addressable. |
| `<text-animator>`, `<type-on>`, `<underline>`, `<text-anchor>` | Animate an optional named span's range, reveal a grapheme count, decorate laid-out lines, or attach a sibling to a measured point. |

Field Unit samples an independently timed moving image inside its monitor. Backdrop effects can sample earlier layers through the compositor. The headless tests exercise the order and coverage of these effects. [Monitor](../../../examples/field-unit/sequences/instrument.xml) · [Backdrop tests](../../../engine/tests/backdrop.rs) · [Native reader](../../../engine/src/project/xml.rs) · [SDK writer](../../../sdk/python/n4/xml.py)

## Editing and verification

Run `just host-native` and open the [template project](../../../examples/signal-atlas/README.md). Change the local end key of the survey transect and render the same local moment in both placements. A camera cut belongs to its placement and can give the two pictures different framing. The [SDK tests](../../../sdk/python/tests/test_xml_authoring.py) exercise typed edits, XML round trips, literal whitespace, rejected revisions and write conflicts.
