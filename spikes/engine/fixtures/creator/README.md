# Creator fixtures

The creator edits run at 960 × 540, 30 fps. The product interaction and paper plate were generated for these trials. The projects add the editorial title, chart and credit in the native compositor.

## Product edit

The original take switches the example audio application’s hum removal on at frame 48. Take B delays the click to frame 78. Each take has its own 48 kHz mono WAV with the matching change in hum. The revised project swaps both assets and lengthens the editable title. Inspect frame 60 for the two different switch states and line breaks. These are reproducible simulated application recordings. A real capture has separate decoder and performance coverage.

## Synthetic chart

Seven invented sounding levels drive an editable SVG line. The axes use a second SVG. The WGSL filter computes grain in local pixel coordinates. The revised edit moves the reveal from frames 18–171 to 24–177 and raises the axes’ ink effect from 16% to 82%. The builder synthesizes the two 48 kHz audio stems from oscillators.

The supported SVG subset has explicit pixel dimensions, path geometry, solid RGB fills, strokes, joins, caps, isolated groups with opacity and ordered linear gradients in explicit user-space coordinates. Local gradient IDs can supply fills or strokes. Unsupported elements and presentation attributes report errors. The chart reveal uses a horizontal wipe in an ascending-time plot. The renderer caches the raw painted layer and its uploaded texture across timing, placement and filter parameter edits. Path-based source assets are immutable for an open snapshot.

## Gradient and wrapped backplate

The [material project](material.json) composes an external [gradient illustration](stereo-field.svg) with a native paragraph. Two overlapping filled paths share a group at 65% opacity, with a fine gradient stroke above them. The rectangle behind the title follows the paragraph’s longest measured line and total laid-out height. Color, horizontal padding and vertical padding are authored. The title’s position still locates the text. CSS inheritance, gradient transforms, object-bounding-box gradients and arbitrary SVG filters remain outside this importer.

## Running the trials

The six-second native Path project reuses Take B. A curved mask clips four overlapping path fills and a separate dry-ink stroke outlines the result. Its replay edits a control point, moves the group, undoes both changes, reopens and verifies the entire export. [Editable source](path.json)

The frosted-caption project puts Take B at the back of the graphics list. One WGSL program blurs the underlying picture horizontally and vertically, while another animates how much of the blurred result the card reveals. Scrub frames 45–120 to see the orange trace cross the caption. [Editable source](frost.json)

Run from `spikes/engine`. Each movie output directory should be new. The generators require Python, Pillow, NumPy and FFmpeg. The prepared sources are included.

```sh
cargo build --locked --bin n4
python3 -B fixtures/creator/verify.py target/debug/n4 /tmp/n4-creator-review --movies
python3 -B fixtures/creator/exposure-check.py --cli target/debug/n4 --out /tmp/n4-exposure-review
python3 -B fixtures/creator/material-check.py target/debug/n4 /tmp/n4-material-review
python3 -B fixtures/creator/path-check.py target/debug/n4 /tmp/n4-path-review
python3 -B fixtures/creator/frost-check.py target/debug/n4 /tmp/n4-frost-review
```

To rebuild the input plates, stems, SVGs and editable projects, run `python3 -B fixtures/creator/build.py`. To rebuild the cut/hold and shutter fixture, run `python3 -B fixtures/creator/exposure-check.py --build`.

The exposure fixture uses a 360° shutter and four equally spaced midpoints inside each output frame. It exposes the animated graphics, holds the selected source picture through the sample interval and resolves the samples in linear light on the GPU. The sample count is bounded to eight, with a 256 MiB limit on the sample array. Frame 8 exposes a rapidly revealed white bar. Frames 10/11 and 12/13 hold the selected source on opposite sides of a hard cut. With exposure absent, inspection and export evaluate the exact integer-frame edit.
