# Editing Signal Atlas

This folder is a complete editable N4 film. Start with [project.proj](project.proj) and the sequence that owns the requested change. The editable title, survey line, status badge and camera cuts all live in plain text. [preview.mp4](preview.mp4) and [poster.jpg](poster.jpg) show the original cut.

## Find the edit

| Creator's request | File and target |
| --- | --- |
| “Change the title” | [survey.xml](sequences/survey.xml), `<text name="Film title">`. Edit its `<copy>`. `&#10;` is the deliberate line break. Check that the new copy fits its 320 px box. |
| “Draw the route faster” | In the same file, find `Survey transect` and move the last `stroke-end` key from 46 to 24. The orange line reaches its destination sooner in both survey placements. |
| “Move the station mark” | The imported [ring](assets/station.svg) is one layer inside the masked `Survey plate` group. Its `position` is measured from the plate's upper-left corner. |
| “Show the status only during the first shot” | In the master, remove the second `V2 transmission` occurrence. Editing [transmission.xml](sequences/transmission.xml) changes both uses. |
| “Give the return shot a different lens” | Adjust the `CAM` cut under the second `survey` in the master, or the `detail` camera in the survey XML. The first occurrence uses the default `wide` camera. |
| “Change the sound or its timing” | The `A1` placements read [score.json](sequences/score.json) and [reprise.json](sequences/reprise.json). They share a 48 kHz PCM [source](assets/pings.wav). Keep the two trims continuous when moving their cut. |

For a variant affecting only one use of a sequence, duplicate its definition with a new filename and change the corresponding placement name. Preserve the shared definition when the edit should reach both uses. Keep ordinary text, geometry and animation in the XML so another editor can inspect them. [generate.py](generate.py) makes the topography, echo field and audio stems.

## Read the clocks

The master declares **24 fps, 960 × 540**. Ranges use integer frames and exclude their end. The last project frame is 191. Indentation gives placement times a parent. In the middle shot, `analysis` starts at project frame 64. `sounding` starts at frame 0 inside it. Its `Instrument` group starts at local frame 8 and the nested `Signal lamp` reaches full opacity at key 4, so the project frame is **64 + 0 + 8 + 4 = 76**. At this rate one picture frame spans 2,000 audio samples.

## Add a shot

For a two-second end card, append `[192,240) V1 end-card` to the master. Create `sequences/end-card.xml`:

```xml
<sequence background="#112e36">
  <text name="End title" position="38 178" font="assets/N4GridSans.ttf"
        size="66" width="884" fill="#eae8d9">
    <copy>END TRANSMISSION</copy>
    <animate attributeName="opacity">
      <key frame="0" value="0"/>
      <key frame="8" value="1"/>
    </animate>
  </text>
</sequence>
```

Render frames 191, 192 and 200 to check the cut and fade. The existing score ends at 192, leaving this card silent. To change the duration independently, edit the master's interval.

## Check the result

Run from the folder containing `project.proj`, with `N4_BIN` pointing at an absolute path to the N4 CLI. These commands work in a copy outside the engine checkout. The [README](README.md) shows how to build the CLI and set the variable.

```sh
"$N4_BIN" validate .
"$N4_BIN" inspect . --placement 'V1:analysis@64/V1:sounding@0' \
  --layer Sounder --layer Instrument --layer 'Signal lamp' --property opacity --frame 76

review=$(mktemp -d)
"$N4_BIN" render . --frames 18,42,76,104,146 --out "$review/frames"
"$N4_BIN" export . --out "$review/film.mp4"
```

For the route edit, render frames **18, 104 and 146** before and after. The line should extend further at 18, the middle echo shot at 104 should be identical, and the line in the returning camera at 146 should extend further too. Use a new review directory for each pass. Keep added media under `assets/` and refer to it from the project root. Record the source and redistribution rights if you intend to share a derivative.
