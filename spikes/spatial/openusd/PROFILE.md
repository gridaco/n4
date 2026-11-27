# OpenUSD rigid mesh import profile

`nothing_openusd::load(path, prim)` opens a composed USD stage at its default pose. `load_at(path, prim, time_code)` samples rigid transforms at an explicit USD stage time code. Both return owned [CPU geometry, parts, named material slots and decoded images](../scene/src/lib.rs). The calling host chooses how its timeline maps to asset time. N4 can direct a rigid part independently at its imported pivot.

| Source feature | Behavior in this profile | Evidence |
| --- | --- | --- |
| Composition, units and axis | Resolves references through OpenUSD. Positions are relative to the selected prim's parent and include the selected root transform. Converts stage units to meters and Z-up to right-handed Y-up. | [Referenced geometry test](tests/rigid.rs) |
| Hierarchy and rigid motion | Preserves the absolute prim path, parent, own triangle range, rest frame and pivot of each visible transformable node. Vertices share asset space. CommonAPI's `xformOp:translate:pivot` defines the rotation center when authored. A DCC object with a hinge origin uses that origin. `load_at` samples OpenUSD's ordered transform stack and stage interpolation, including between source keys. | [Centimeter Z-up animated hinge](tests/data/hinge.usda) and [Blender recorder](../../examples/field-unit/assets/field.usdc) |
| Geometry and assignments | Reads triangles and simple quads, including concave quads with a valid interior diagonal. Reads authored normals or computes flat normals. Bound subsets persist as per-triangle material indices and named slots. | [Three-face enamel swatch](tests/data/enamels.usda) and [studio DCC regression](../../engine/tests/usd_asset.rs) |
| UVs | Reads a single coordinate set per mesh with constant, uniform, vertex, varying or face-varying interpolation. Resolves indexed UV primvars and keeps position/normal/UV/material seams. Coordinates retain USD's lower-left origin. | [Indexed seam with known RGBA quadrants](tests/data/seam.usda) |
| Materials and images | Selects the universal opaque `UsdPreviewSurface` network. Imports existing constant values and diffuse, normal, roughness, metallic and occlusion map connections through `UsdUVTexture` and `UsdPrimvarReader_float2`. Follows a forwarded primvar name. Preserves selected channels, declared color interpretation, scale and bias. Resolves external files and package members with Ar and decodes PNG/JPEG to top-down RGBA8 once per resolved source. | [RGB and G map using one image](tests/rigid.rs), [Packaged UV sample](tests/data/seam.usdz) |
| Visibility and purpose | Skips invisible and guide/proxy surfaces. Cameras and lights are host-owned. Animated visibility, point deformation, time-varying UVs, skinning, blend shapes and point instancing produce diagnostics. | [Traversal](native/usd_mesh.cpp) and [rig rejection](tests/rigid.rs) |
| Unsupported inputs | Reports missing textures, unsupported networks, UV sets and wrap modes, subdivision, invalid topology and unsupported geometry. The shader/input or prim path identifies the first failure. Use polygonal triangles/simple quads and author `subdivisionScheme = "none"`. | [Missing texture](tests/data/connected-surface.usda), [default subdivision diagnostic](tests/rigid.rs) |
| MaterialX | MaterialX networks fall outside the supported universal Preview Surface profile. | [Fixture inventory](fixtures/README.md) |

The sampled texture profile requires authored `repeat` on both axes and an explicit `raw` or `sRGB` interpretation. USD's `useMetadata` wrapping falls back to black, and its automatic color space depends on image metadata and channel depth. The importer diagnoses these modes until the texture loader and sampler implement their semantics. [Preview Surface specification](https://openusd.org/release/spec_usdpreviewsurface.html) Straight image pixels remain separate from premultiplied compositing outputs. Procedural graphs, animated material inputs, arbitrary UV transforms, alpha-blended triangle intersections and round-trip USD writing require separate coverage.

## Local SDK and tests

Run in `spikes/`. On the test machine, macOS arm64, the SDK is conda-forge OpenUSD 26.05 with Python 3.12. Provision that development prefix with the following command.

```sh
micromamba create --yes --prefix "$PWD/engine/target/openusd-sdk" --channel conda-forge openusd=26.05 python=3.12
```

The default `sdk` feature discovers this prefix or accepts an explicit `OPENUSD_DIR`. A clean development checkout with no SDK builds the adapter stub. The `require-sdk` feature makes missing headers a build error so the native test job exercises the bridge. Use `--no-default-features` to exercise the SDK-free path even on a provisioned machine.

```sh
cargo test --locked -p nothing-openusd --features require-sdk
cargo test --locked -p nothing-openusd --no-default-features
```

On a machine with a Metal device, `cargo test --locked -p n4-engine --test usd_asset` checks the host boundary. The engine opens a project, the test moves its USD source before the first draw, and the mesh still renders. After a failed edit, the test checks the accepted revision, the pixels and reuse of the uploaded buffers.

The native bridge loads the SDK’s shared libraries and plugins. Cargo exposes the development library directory to a direct dependent’s build script as `DEP_NOTHING_OPENUSD_LIBDIR`. N4 adds it to its executable’s runtime search path. The verified environment occupies about 1.1 GiB including SDK tools, Python and Qt. The relocatable reader and its qualification commands are described below.

## Standalone macOS qualification

The arm64 CPU reader bundles its dynamic libraries, OpenUSD plugin registry, generated schemas and available native dependency notices. The optimized package measured 46 MiB on disk and about 11 MiB compressed. Each Mach-O binary uses a bundle-relative load path and an ad-hoc code signature. The demonstrated scope is a development reader distribution. The full editor, notarization and signing for general distribution have separate release requirements.

Build from `spikes/` using the pinned development SDK. Both output paths below assume a new destination.

```sh
cargo build --release --locked -p nothing-openusd --features require-sdk --example inspect_stage
python3 spatial/openusd/tools/package_macos.py engine/target/release/examples/inspect_stage \
  engine/target/openusd-sdk /tmp/nothing-usd-reader
python3 spatial/openusd/tools/verify_macos_bundle.py /tmp/nothing-usd-reader \
  --sdk engine/target/openusd-sdk \
  --stage examples/field-unit/assets/field.usdc /World/Recorder 29928 229 10 0 \
  --stage spatial/openusd/tests/data/seam.usdz /Card 2 1 1 1
COPYFILE_DISABLE=1 tar -czf /tmp/nothing-usd-reader-macos-arm64.tar.gz -C /tmp nothing-usd-reader
```

The verifier copies the reader to another directory, checks the Mach-O library links and signatures, clears the process environment and denies file writes, network and development paths. It checks that reads of an unbundled control file and the SDK header fail. It then opens a composed USDA stage and compares the import counts for the supplied instrument and textured package. A previous run on one arm64 Mac is recorded in the [local receipt](verification/macos-arm64.json).

For an independent install, unpack the archive on a separate Apple Silicon Mac and run `./nothing-usd-reader/check-reader.sh`. The shell check ships with a rigid USD stage and a textured USDZ. It only needs the system shell and utilities. The [manual two-runner workflow](../../../.github/workflows/usd-reader-macos.yml) builds the package and passes an archive to a fresh arm64 runner. The second job opens both fixtures and compares the independent Blender import to the producer. It receives the bundle and fixtures without a repository checkout or SDK installation. The local receipt records same-machine relocation and isolation. The independent-host gate requires an actual second run.
