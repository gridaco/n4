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
| `gif/restore.gif` | The [Python generator](gif/generate.py) writes four original indexed patches on an 8×4 canvas. The temporary green patch tests restoration of both red and transparent pixels. Local palette, background disposal and unequal delays have independent expected values. The bytes and fixture artwork are CC0. |
| `webp/` | The [generator](webp/generate.py) draws an original 24×16 decal with four alpha levels, builds its linear RGB ICC profile, and flips it for a two-frame animation. The artwork, synthetic profile and generated images are dedicated to CC0. |

For films intended to be edited or watched, open the [canonical examples](../../examples/README.md).

### Regenerate the WebP controls

Run `python3 spikes/engine/fixtures/webp/generate.py` from the repository root. The checked-in bytes were generated with Pillow 11.3.0, libwebp 1.5.0 and LittleCMS 2.17. `decal.png` contains the authored samples. The `opaque-lossy.png` and `alpha-lossy.png` witnesses contain libwebp's decode of each lossy input. `linear-rgb.png` contains LittleCMS's conversion of the linear samples to sRGB. The tests compare every pixel after premultiplying, require exact alpha, and allow one RGB code value for lossy reconstruction or ICC conversion. The two-frame animation also exercises variable holds and color conversion. The original CC0 `patches.webp` paints three 2×2 opaque patches on a 4×2 transparent canvas with 40/80/20 ms holds. After red clears on disposal, the left half must show the footage underneath the green frame. Its container omits the alpha flag.

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
