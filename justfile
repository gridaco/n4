set positional-arguments

# List canonical example projects from the repository root.
projects:
    @for project in spikes/examples/*/project.proj; do dirname "$project"; done

# Open the bundled eight-second motion example in the native viewer.
demo: (play (justfile_directory() / "spikes/examples/signal-atlas"))

# Open a project directory or file, relative to the caller's working directory.
[no-cd]
play project *args:
    @exec {{quote(justfile_directory() / "spikes/engine/Play Timeline.command")}} "$@"

# Browse the authored timeline, property curves and the native movie in one window.
[no-cd]
inspect project *args:
    @exec {{quote(justfile_directory() / "spikes/engine/Play Timeline.command")}} --inspector "$@"

# Build the headless host and command-line renderer.
host-native:
    @cd spikes/engine && LIBRARY_PATH="$(sdl2-config --prefix)/lib${LIBRARY_PATH:+:$LIBRARY_PATH}" cargo build --locked --features host-audio,native-decode --bin n4-host --bin n4

# Watch a project without Electron. Prints a JSON line for each reload at the parked frame.
[no-cd]
watch-project project frame="0": host-native
    @exec node {{quote(justfile_directory() / "spikes/host/watch-project.mjs")}} "$@"

[private]
host-deps:
    @cd spikes/host && pnpm install --frozen-lockfile

# Build the native sidecar and the embeddable viewer harness.
host-build: host-native host-deps
    @cd spikes/host && pnpm run build

# Open a project in the Electron harness, e.g. just viewer spikes/examples/signal-atlas --frame=42
[no-cd]
viewer project *args: host-build
    @project_path="$(realpath "$1")" && shift && cd {{quote(justfile_directory() / "spikes/host")}} && exec pnpm run start "$project_path" "$@"

# Run the class-level tests, real inspection fixtures and sidecar pixel-parity checks.
host-test: host-native host-deps
    @cd spikes/host && pnpm run typecheck && pnpm test && node --test kit/test/timeline-native.ts && node test/native-contract.mjs && node --test test/native-watch.mjs test/native-editor.mjs test/live-reload.mjs test/selective-invalidation.mjs

# Check a reused sequence, audio gain and camera cut against a fresh render.
test-invalidation: host-native host-deps
    @cd spikes/host && node --test test/selective-invalidation.mjs

# Exercise geometry, session commands and saved track state without a window.
editor-test:
    @cd spikes/engine && LIBRARY_PATH="$(sdl2-config --prefix)/lib${LIBRARY_PATH:+:$LIBRARY_PATH}" cargo test --locked --features host-audio,native-decode --lib --test editor_cli --test editor_svg --test editor_tracks --test editor_geometry
