# Engine regression inputs

The media and authored fixture content in this directory are original N4 work dedicated to [CC0 1.0](../../../LICENSES/CC0-1.0.txt). The shared Gaussian kernel is a link to engine source covered by the [software license](../../../LICENSE). Contributors can run the tests with the committed inputs. Regeneration is optional.

| Input | Origin and purpose |
| --- | --- |
| `numbered.mkv`, `numbered-vfr.mp4` | Locally generated frame IDs and binary cell patterns for decoding and seek verification. See [cue sheet](numbered.md), [frame generator](make_numbered.py) and [VFR generator](make_vfr.sh). |
| `av-cut/` | [Generator](av-cut/generate.py) paints an oscilloscope, source-frame numbers and cue marks. The two PCM stems are synthesized. Used for picture/audio alignment across a cut. |
| `creator/` | [Generator](creator/build.py) draws a fictional audio application and synthetic sounding chart, paints a paper texture and synthesizes audio stems. Geometry, mock data, SVG and effects are authored in the checkout. [Fixture guide](creator/README.md). |
| `screen-capture/` | Original [single-page mock application](screen-capture/app.html). The H.264 screen recording and twelve PNG witnesses show only that application. The half-size draft is transcoded from that recording. [Capture](screen-capture/record.cjs), [packaging](screen-capture/package.py) and [proxy](screen-capture/make_proxy.py) reproduce the procedure. Browser capture timing varies between runs. |
| `fonts/` | N4 Grid Sans is built from [original glyph strokes](fonts/build_font.py), including the shaping test glyphs. |
| `playback/` | Generates a longer workload with FFmpeg test patterns for optional performance studies. [Instructions](playback/README.md). |
| `knot-art.json` | Original parameterized cubic-curve articulation data for the geometry regression. |

For films intended to be edited or watched, open the [canonical examples](../../examples/README.md).

### Re-record the browser fixture

Only fixture maintainers need Chrome, Node, Playwright and Pillow for this step. From the repository root, capture a fresh three-minute input in a temporary directory. The script intercepts a private test origin and serves exactly the local HTML and N4 Grid Sans files. Each click updates the mock application. Chromium supplies the presentation timestamps.

```sh
work=$(mktemp -d)
npm install --prefix "$work" playwright
PLAYWRIGHT_MODULE="$work/node_modules/playwright" node spikes/engine/fixtures/screen-capture/record.cjs "$work/raw" 180
python3 spikes/engine/fixtures/screen-capture/package.py "$work/raw" "$work/candidate"
python3 spikes/engine/fixtures/screen-capture/make_proxy.py "$work/candidate/recording.mp4" "$work/candidate/draft.mp4"
```

Keep the recording, half-size proxy, capture index and PNG witnesses from the same run together. The picture checks use the barcode in the page footer to independently verify source identity after seeking.
