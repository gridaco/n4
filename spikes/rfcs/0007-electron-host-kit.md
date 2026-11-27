# RFC 0007 — Embedding the N4 viewer in Electron

**Status** — Implemented as a development kit with a headless editor session. [Run and embed the kit](../host/README.md). The macOS harness presents Rust-owned preview and edit state. A parent application should be able to supply its own window, project picker and chrome, mount the same timeline and preview, and dispose of them when the document closes.

## Native boundary

Rust already knows which occurrence of a sequence is on which lane, the placement's local and film ranges, its layers, and its property keys. Rust also renders frames, runs a bounded playback queue and samples filmstrip thumbnails. The extracted Rust playback controller starts audio after the first picture is submitted and handles stalls and cut boundaries. Each host installs a reopened document with its parked picture. The SDL viewer remains the comparison instrument. The host and inspector open authored projects through the same loader. The Rust editor session owns selection, edit history and save. [Inspection model](../engine/src/inspect.rs) · [Playback controller](../engine/src/host/controller.rs) · [Reopen](../engine/src/bin/player/reopen.rs) · [Editor session](../engine/src/editor/mod.rs)

## Layout

```text
spikes/
  engine/
    src/host/                 Rust viewer session, commands, events
    src/bin/n4-host.rs        Persistent process for one open document
    src/bin/player/           Existing SDL viewer and native trials
  spatial/                    Existing scene, OpenUSD and wgpu crates
  host/
    package.json              Private TS workspace
    kit/                      Reusable TS package, multiple entry points
      src/client/             Typed session client and wire validation
      src/timeline/           TimelineController, layout and hit testing
      src/view/               Canvas timeline, keyboard/pointer bindings, preview
      src/electron/           Main-process launcher and narrow preload bridge
      test/                   Node interaction tests and real service contract tests
    harness/                  Private Electron app consuming kit exports
      src/{main,preload,renderer}/
      test/                   Window, presentation and lifecycle trials
```

Use separate package entry points for the portable client/controllers, browser views, Electron main and Electron preload. A Node test can import the controller without loading Electron, constructing a canvas or emulating the DOM. The harness imports the same package entry points available to the parent. It owns window creation, file grants, menus and optional React chrome. The kit owns mounting, input and disposal of its surfaces. The Python SDK remains the authoring SDK. These directories can start as modules in the two indicated packages and the existing engine crate.

## Contract to prove

| Boundary | Responsibility |
| --- | --- |
| Rust → client | An accepted session revision supplies rational frame rate, duration, half-open frame ranges, placements, shared definitions and key tracks. Occurrence addresses are scoped to the revision. The engine observes project inputs. Protocol v4 sends `{event: "inputsChanged", revision}` on the control pipe after a save settles. A successful reopen installs the validated candidate and its parked picture together, with independent picture and audio damage in project frames. The engine also reports occurrence marks and invalid samples of isolated placement thumbnails. Hosts apply this evidence across the two named revisions. A rejection retains the accepted revision. |
| Client → Rust | Open a host-granted project, inspect, seek in project frames, play, pause, request thumbnail samples, reopen and close. A drag can supersede earlier targets. Release requests the exact frame. Rust owns the media clock, seek scheduling, decoding, composition and audio output. |
| Timeline → host | The controller owns viewport, expanded rows, selection, zoom, drag capture and keyboard navigation. It consumes an inspection snapshot and emits intents such as seeking to a key's film frame. One layout result serves drawing and hit testing. A local cursor follows input immediately. The delivered picture and its frame number advance independently. |
| Presenter ↔ Rust | A frame carries session, revision, transport generation, frame index, format, size and a lease. A bounded number may be outstanding. The host reports submission for display and releases storage when safe. Abandoned, obsolete and disconnected consumers must return capacity. Thumbnails have lower priority and a separate delivery budget. |

Start with a Rust child launched once by Electron main and a small framed protocol over pipes. Keep typed control messages and binary frame payloads on separate channels so a busy picture stream cannot delay pause or seek. Reserve stdout/stderr roles explicitly and keep diagnostics out of frame data. A handshake verifies matching protocol builds. Check representative Rust messages against TS decoding, including malformed messages. During incubation, update both ends together. Expose only granted project operations through the isolated preload. The parent supplies an engine executable locator, a granted project and two mount points. It can also consume the session without mounting the timeline. [Electron context isolation](https://www.electronjs.org/docs/latest/tutorial/context-isolation)

The first presenter uses bounded RGBA copies for correctness and as a measured control. The existing SDL player already uploads CPU-visible frames into a streaming texture. An early Apple Silicon experiment should price each step against that baseline. If copies consume the budget, test native shareable surfaces end to end, including cross-process handle transfer, GPU completion and final reference release. Electron's experimental shared-texture import is a candidate. A successful import alone proves little about pacing or fidelity. Both presenters feed a normal page surface with room for overlays. [Native presenter](../engine/src/bin/player/main.rs) · [Electron shared textures](https://www.electronjs.org/docs/latest/api/shared-texture)

## Delivery order and exit tests

1. **Run the session without a window.** Use a Node client to open Signal Atlas, inspect a nested occurrence, seek to film frame 42, and compare the returned pixels with the native engine for that frame. Rapid seeks must settle exactly. Reject stale frames after reopen and keep the accepted project usable after a malformed edit. Closing twice or losing the client must release workers and leases.
2. **Qualify the Electron picture path early.** Mount one preview canvas with a visible DOM overlay. Replay the same forward/reverse drag in SDL and Electron on the same machine, build profile, film, resolution and display. Record p95 picture hold, target lag, exact-release latency, outstanding buffers and memory across resize/reopen. Distinguish frame ready, canvas submission and any measured display timing. Gate on exact landing, bounded memory and agreement on color, alpha and Retina sizing. Use paired native timings and a hands-on scrub to agree the acceptable host overhead before polishing the timeline. [Performance guide](../../docs/performance.md)
3. **Port the read-only interaction.** Give the TS class an inspection snapshot, viewport dimensions and synthetic pointer/keyboard input. In Node, inspect the sounding sequence's signal lamp at frame 76. Switch between uses of the survey artwork and seek the reprise's cut at 140. Assert occurrence, ruler coordinates, hit result and emitted project frame. The canvas paints the time ruler, lanes, keys, thumbnails and waveform peaks. HTML controls draw the sidebar and playhead from the same layout. Use the headless controller regression for those interactions. Exercise the Electron window for host-specific focus or presentation issues.
4. **Prove playback and embedding.** Use the native audio output and its sample clock. Exercise play/pause across cuts, seek during playback, stall and resume, hide/show, renderer reload, service failure and clean window close. Verify the host presents the right generation before starting sound. Compare with native playback at authored speed. Run the mixed 2D/3D Field Unit film through the same API. Build a minimal parent-consumer test that imports only kit exports, mounts, unmounts and reopens. Launch one locally packaged harness to verify the sidecar and native dependencies are discoverable outside the source checkout.

The editor milestone starts with live project reload and then introduces a headless editor session for authored edits. Preview picking, selection, track state and persistence need contracts that another host or a CLI can exercise. Signal Atlas exercises edit-damage highlights with two placements of shared artwork. [Editor backlog](../docs/research/video-editor/toolings.md#editor-milestone) · [Live-edit demo](../docs/research/video-editor/live-edit-demo.md)
