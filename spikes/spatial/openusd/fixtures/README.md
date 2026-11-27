# OpenUSD fixture inventory

The shared reader suite works offline with the small N4-authored controls below once the SDK is installed. Their source and corresponding packaged assets are dedicated to the public domain under [CC0 1.0](../../../../LICENSES/CC0-1.0.txt).

| Input | What it establishes |
| --- | --- |
| [Animated hinge](../tests/data/hinge.usda) | Parent hierarchy, units, Z-up conversion, pivot and time-sampled transforms. The subdivision diagnostic test removes the explicit polygon scheme from this fixture. |
| [Indexed card](../tests/data/seam.usda), [quadrants](../tests/data/quadrants.png), [package](../tests/data/seam.usdz) | Two triangles share a position with different UVs. The original 2×2 swatch is red, green, blue and half-alpha white. The color and roughness connections read it with different color and channel semantics. The USDZ contains only the card and this image. |
| [Packed channels](../tests/data/packed-channels.usda) | A sublayer of the indexed card binds its raw texture to normal, occlusion and metallic inputs. |
| [Enamel swatch](../tests/data/enamels.usda) | Three triangular faces share smooth normals across material subset boundaries. The center face has a cobalt clearcoat. |
| [Connected surface](../tests/data/connected-surface.usda) | A deliberately missing texture identifies the exact material input in the error. |
| [Field Unit](../../../examples/field-unit/README.md) | A product-scale, original DCC export. The integration suite checks its assembly, child pivot and addressable material slots. |

The CPU suite also writes temporary stages to exercise references, rebinding after a file edit and an unsupported skeletal binding. [Regression tests](../tests/rigid.rs)

## Production-scale qualification

The small fixtures isolate individual reader features. To qualify a large stage, start with one camera and the base geometry. Record unresolved dependencies, unsupported geometry and material types, visible discrepancies, cold-open time, peak memory, seek latency and final-frame time. Repeat with higher-resolution textures and any procedural dependencies. Record the stage revision and licenses before adding a case to the public suite.
