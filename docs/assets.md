# Public examples, fixtures and private studies

The Git checkout contains two kinds of redistributable inputs.

| Location | What belongs here | Evidence of origin |
| --- | --- | --- |
| [Examples](../spikes/examples/README.md) | An editable, portable production worth watching. Include the master, artwork, inputs, a representative frame or export, and a concrete edit to try. | Each project's README identifies its inputs and CC0 dedication. A public project opens from a copy with its dependencies beside it. |
| [Engine fixtures](../spikes/engine/fixtures/README.md) and [spatial fixtures](../spikes/spatial/openusd/fixtures/README.md) | Small controls with a known frame, sample, geometric or material property a test can assert. | Preserve an original generator, source drawing or documented external CC0 origin and the expected result. |

Shared example artwork and test media must be CC0. Record the author and source or generator, the terms, and any derived files, including previews, textures and fonts. A free download, permissive software license, attribution, or link to a video does not establish CC0. For an original contribution the rights holder must be willing to dedicate it. Engine and SDK code, documentation and reusable kernels keep the repository's Apache license. Dependency licenses continue to apply to those dependencies. [Software license](../LICENSE) · [CC0 terms](../LICENSES/CC0-1.0.txt)

The [media inventory](media.sha256) pins the bytes of the reviewed binary assets and authored SVG/USD assets. When an input changes, review its provenance and update its entry (`shasum -a 256 path/to/asset` from the root). After staging, run `python3 scripts/check-assets.py`. CI checks tracked media hashes, relative symlinks and local documentation paths. This detects additions and replacements. Rights review belongs to the contributor and reviewer.

## Working from a reference locally

Put experiments, footage, comparison exports and notes in `.local/`. For example, `just inspect .local/examples/my-cut` opens a private project through the same entry point. This directory is ignored. Public builds, examples, CI and normal regression suites must work with it absent. Keep the source film, publisher-specific notes and comparison renders together in the private study. For the public suite, describe the production technique, use documented CC0 inputs and retain a test for the specific edit or rendering behavior.
