# Contributing to N4

Use `spikes` as the base branch for changes and pull requests. The alpha target is macOS on Apple Silicon with Metal. Start by opening Signal Atlas through the [quickstart](README.md#run-the-first-film). FFmpeg, SDL2 and libwebp come from Homebrew. Rustup selects the repository's pinned compiler when Cargo runs under `spikes/`. OpenUSD, Blender and Electron are optional for the first film.

## Make a change

1. Read the relevant [contract](docs/README.md). For a new primitive, describe the creator's edit, its precedent and the component that owns it.
2. Edit a copy of an example under `.local/` if you want to experiment. The viewer and CLI accept a project directory there. Only changes to the shared examples belong in their tracked directories.
3. Exercise the change at its native entry point. Use a headless Rust, CLI or SDK test for an engine contract. Test the Electron window when the behavior depends on the window or presentation lifecycle. Include a regression case for a bug.
4. Update the affected spec and include the commands and results in your pull request. For timing claims, identify the machine, build profile, resolution and interaction measured.

## Check your work

```sh
(cd spikes && cargo fmt --all --check)
just editor-test
```

The editor suite runs without a window and uses Metal. For host changes install Node 24 and the pinned pnpm version, then run `just host-test`. For import changes follow the [OpenUSD reader instructions](spikes/spatial/openusd/PROFILE.md#local-sdk-and-tests). CI builds the headless engine and host and runs a portable subset of the contracts. Full graphics and playback qualification uses a Mac with a Metal device. [Regression guide](docs/tests.md) · [Asset policy](docs/assets.md)

For a design note, name the production technique and the edit a creator needs to make. Cite the format or tool’s upstream specification when its behavior informs the contract. An original CC0 example or a small fixture should let someone verify the claim from a checkout. Keep film-by-film analysis, source copies and private device receipts under `.local/`.

For a bug report include a command or edit, the expected result, the observed frame or interval, and your OS and tool versions. A minimal authored scene is often enough. Public reproductions should use the included fixtures or other documented CC0 assets.
