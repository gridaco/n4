# Native engine

The Rust engine evaluates authored projects at a requested frame, renders their layers with Skia and wgpu, mixes audio and exports H.264/AAC with FFmpeg. The same engine powers the headless CLI, SDL viewer and Electron sidecar. [Project and XML syntax](../../docs/project-format.md) · [Example catalog](../examples/README.md) · [Headless editor contract](../rfcs/0008-editor-session.md)

## Build and run

The alpha development target is Apple Silicon macOS with Metal. Install Xcode Command Line Tools, Rust through rustup, and Homebrew. Rustup reads the pinned toolchain automatically when Cargo runs inside `spikes`. The first build downloads Rust dependencies and the Skia binary for the host.

From the repository root:

```sh
brew install ffmpeg sdl2 webp just
just host-native
spikes/engine/target/debug/n4 validate spikes/examples/signal-atlas
spikes/engine/target/debug/n4 render spikes/examples/signal-atlas --frames 42,84 --out /tmp/n4-atlas-frames
spikes/engine/target/debug/n4 export spikes/examples/signal-atlas --out /tmp/n4-atlas.mp4
```

Use a fresh destination for export. FFmpeg/ffprobe must be on `PATH`. The native decoder links the Homebrew FFmpeg libraries. If using another install, set `FFMPEG_DIR` to its prefix containing `include` and `lib`. Animated WebP links libwebpdemux from Homebrew or the `WEBP_DIR` prefix. GIF decoding uses a Rust crate. SDL2 supplies the native audio and window. The CLI alone also builds with `cd spikes && cargo build --locked -p n4-engine --bin n4`. Building and validating 2D projects works without an OpenUSD SDK. Field Unit exercises the optional spatial reader. [OpenUSD setup](../spatial/openusd/PROFILE.md#local-sdk-and-tests)

## Preview and edit

```sh
just demo
just inspect spikes/examples/signal-atlas
just watch-project spikes/examples/signal-atlas 42
```

In the SDL window, drag the ruler to seek, press Space to play or pause, and use the arrow keys to step by a frame. Home and End jump to the endpoints. An external save reloads the accepted project at its parked frame. A failed load reports the diagnostic and leaves the previous picture available. Use F5 or Command-R to retry manually. `watch-project` uses Node 24 and prints the same headless revision changes. [Electron controls](../host/README.md) · [Rust watcher](src/project_watch.rs)

## Engine boundaries

| Component | Responsibility |
|---|---|
| `project`, `plan` | Resolve the base V1 shot, then each active V2 graphics placement in paint order. Each placement has its own clock from the first frame of its half-open interval and can cross a V1 cut. In a nested sequence, the inner V2 paints before its parent's V2. Project-wide graphics paint last on the project clock. |
| `media` | Index displayed timestamps and resolve the source picture. GIF and WebP adapters reconstruct transparent animation canvases on a shared source clock. FFmpeg decodes movies in presentation order and seeks on discontinuities. One reader bounds its recently decoded pictures across sources. Draft preview and filmstrips can use an explicit proxy after exact onset and terminal-hold validation. Export opens the originals. |
| `graphics`, `render` | Shape wrapped text, paint native paths and import the supported SVG subset with Skia. The wgpu compositor runs creator WGSL and applies geometric or SVG alpha masks to a layer or an isolated group. The current interop path reads the finished RGBA frame back to CPU memory. |
| `export` | Mix clips on a 48 kHz stereo sample clock and stream native frames to FFmpeg. Inspection and export call the same render path. |
| `preview` | Scrubbing and filmstrips each retain the latest request. A media shot under an active V2 placement supplies a composed thumbnail. Media shots with no shot graphics or active placement use source thumbnails. Native backgrounds also use the composed picture. Continuous playback has a separate bounded, sequential frame queue. |
| `session` | Keep a playable revision while commands and direct file edits are validated. Saving preserves relative paths. External producers can publish one immutable result to multiple trimmed uses. |

For a split master, `n4 inspect PROJECT` shows the timeline occurrences, definitions and keyframes. `n4 validate PROJECT --detail ID` reads a proposed replacement detail from stdin and checks all its placements before you save. `n4 text-metrics PROJECT --detail ID --layer PARENT --layer LABEL` returns bounds, baselines and UTF-8 source ranges. The Python authoring SDK uses the same commands. [SDK](../sdk/python/README.md)

Run `just editor-test` on a Metal-capable Mac for the Rust editor and picking checks. `just host-test` adds headless Node tests for revision changes, the sidecar and the timeline. [Regression guide](../../docs/tests.md) · [Performance](../../docs/performance.md)
