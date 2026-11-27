// Resolve USD composition through Pixar's library. Rust takes ownership of the CPU mesh.
#include <pxr/pxr.h>
#include <pxr/base/gf/matrix4d.h>
#include <pxr/base/gf/vec3f.h>
#include <pxr/base/tf/errorMark.h>
#include <pxr/usd/ar/asset.h>
#include <pxr/usd/ar/resolver.h>
#include <pxr/usd/usd/primRange.h>
#include <pxr/usd/usd/stage.h>
#include <pxr/usd/usdGeom/mesh.h>
#include <pxr/usd/usdGeom/metrics.h>
#include <pxr/usd/usdGeom/pointInstancer.h>
#include <pxr/usd/usdGeom/primvarsAPI.h>
#include <pxr/usd/usdGeom/xformCache.h>
#include <pxr/usd/usdShade/materialBindingAPI.h>
#include <pxr/usd/usdShade/shader.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <deque>
#include <cstdio>
#include <limits>
#include <map>
#include <stdexcept>
#include <string>
#include <vector>

PXR_NAMESPACE_USING_DIRECTIVE

namespace {
struct Vertex {
    float position[3], normal[3];
    uint32_t material;
    float uv[2];
};
struct TextureMap {
    int32_t image = -1;
    uint32_t channel = 0, srgb = 0;
    float scale[4]{}, bias[4]{};
};
struct Image {
    const char *source;
    const char *data;
    size_t size;
};
struct Part {
    const char *path;
    int32_t parent;
    uint32_t first_index, index_count;
    float frame[16], pivot[3];
};
struct Material {
    float diffuse[3], emissive[3], specular[3];
    float metallic, roughness, clearcoat, clearcoat_roughness;
    uint32_t use_specular_workflow;
    const char *slot;
    TextureMap maps[5];
};
struct View {
    const Vertex *vertices;
    size_t vertex_count;
    const uint32_t *indices;
    size_t index_count;
    const Material *materials;
    size_t material_count;
    const Image *images;
    size_t image_count;
    const Part *parts;
    size_t part_count;
};

struct ImportError {
    std::string message;
};
void require(bool ok, const std::string &message) {
    if (!ok)
        throw ImportError{message};
}
template <typename V> void copy3(const V &v, float *out) {
    for (int i = 0; i < 3; ++i) {
        require(std::isfinite(v[i]) && std::abs(v[i]) <= std::numeric_limits<float>::max(),
                "non-finite or oversized mesh/material component");
        out[i] = static_cast<float>(v[i]);
    }
}
template <typename T> T input(const UsdShadeShader &shader, const char *name, T fallback) {
    const auto attr = shader.GetInput(TfToken(name));
    if (!attr)
        return fallback;
    require(!attr.HasConnectedSource() && !attr.GetAttr().ValueMightBeTimeVarying(),
            shader.GetPath().GetString() + ": " + name + " must be constant");
    T value;
    require(attr.Get(&value), std::string("cannot read material input ") + name);
    return value;
}

struct Reader {
    GfMatrix4d relative_to_parent;
    double meters;
    bool z_up;
    UsdTimeCode time;
    UsdGeomXformCache transforms;
    std::vector<Vertex> vertices;
    std::vector<uint32_t> indices;
    std::vector<Material> materials;
    std::map<SdfPath, uint32_t> material_ids;
    std::vector<TfToken> material_uvs;
    std::deque<std::string> strings;
    std::vector<std::shared_ptr<const char>> image_buffers;
    std::vector<Image> images;
    std::map<std::string, uint32_t> image_ids;
    std::vector<Part> parts;
    std::map<SdfPath, int32_t> part_ids;

    const char *keep(std::string value) {
        strings.push_back(std::move(value));
        return strings.back().c_str();
    }

    uint32_t image(const UsdShadeShader &shader, const std::string &where) {
        const auto file = input(shader, "file", SdfAssetPath());
        const auto path = file.GetResolvedPath();
        require(!path.empty(), where + ": cannot resolve texture " + file.GetAssetPath());
        const auto known = image_ids.find(path);
        if (known != image_ids.end())
            return known->second;
        auto asset = ArGetResolver().OpenAsset(ArResolvedPath(path));
        require(asset && asset->GetSize(), where + ": cannot open texture " + path);
        auto bytes = asset->GetBuffer();
        require(bool(bytes), where + ": cannot read texture " + path);
        const auto index = static_cast<uint32_t>(images.size());
        images.push_back({keep(path), bytes.get(), asset->GetSize()});
        image_buffers.push_back(std::move(bytes));
        image_ids[path] = index;
        return index;
    }

    bool map(const UsdShadeShader &surface, const char *name, bool rgb, TextureMap &result,
             TfToken &uv_set) {
        auto binding = surface.GetInput(TfToken(name));
        if (!binding || !binding.HasConnectedSource())
            return false;
        std::string where = surface.GetPath().GetString() + ".inputs:" + name;
        const auto producers = binding.GetValueProducingAttributes();
        require(producers.size() == 1, where + ": expected one UsdUVTexture output");
        const auto output = producers[0];
        const UsdShadeShader tex(output.GetPrim());
        TfToken id;
        require(tex && tex.GetIdAttr().Get(&id) && id == TfToken("UsdUVTexture"),
                where + ": expected UsdUVTexture");
        const auto field = output.GetBaseName();
        const std::vector<TfToken> channels{TfToken("rgb"), TfToken("r"), TfToken("g"),
                                            TfToken("b"), TfToken("a")};
        auto channel = std::find(channels.begin(), channels.end(), field);
        require(channel != channels.end() && (rgb == (field == TfToken("rgb"))),
                where + ": unsupported texture output " + field.GetString());
        result.channel = uint32_t(channel - channels.begin());
        result.image = image(tex, where);
        auto colorspace = input(tex, "sourceColorSpace", TfToken("auto"));
        require(colorspace == TfToken("raw") || colorspace == TfToken("sRGB"),
                where + ": sourceColorSpace " + colorspace.GetString() +
                    " unsupported; author raw or sRGB for this image");
        result.srgb = colorspace == TfToken("sRGB");
        const auto scale = input(tex, "scale", GfVec4f(1));
        const auto bias = input(tex, "bias", GfVec4f(0));
        for (size_t k = 0; k < 4; ++k) {
            require(std::isfinite(scale[k]) && std::isfinite(bias[k]),
                    where + ": nonfinite scale or bias");
            result.scale[k] = scale[k];
            result.bias[k] = bias[k];
        }
        for (auto wrap : {"wrapS", "wrapT"}) {
            auto mode = input(tex, wrap, TfToken("useMetadata"));
            require(mode == TfToken("repeat"),
                    where + ": " + wrap + " " + mode.GetString() +
                        " unsupported; this profile supports authored repeat");
        }
        const auto st = tex.GetInput(TfToken("st"));
        require(st && st.HasConnectedSource(), where + ": texture needs a float2 primvar reader");
        const auto coordinates = st.GetValueProducingAttributes();
        require(coordinates.size() == 1, where + ": expected one texture coordinate source");
        UsdShadeShader reader(coordinates[0].GetPrim());
        require(reader && reader.GetIdAttr().Get(&id) && id == TfToken("UsdPrimvarReader_float2"),
                where + ": expected UsdPrimvarReader_float2");
        auto names = reader.GetInput(TfToken("varname")).GetValueProducingAttributes();
        require(names.size() == 1, where + ": texture coordinate primvar is unresolved");
        std::string str;
        TfToken token;
        require(!names[0].ValueMightBeTimeVarying() && (names[0].Get(&token) || names[0].Get(&str)),
                where + ": invalid UV primvar name");
        if (!str.empty())
            token = TfToken(str);
        require(!token.IsEmpty() && (uv_set.IsEmpty() || uv_set == token),
                where + ": multiple UV sets unsupported");
        uv_set = token;
        return true;
    }

    uint32_t material(const UsdShadeMaterial &mat) {
        const SdfPath path = mat ? mat.GetPath() : SdfPath();
        const auto known = material_ids.find(path);
        if (known != material_ids.end())
            return known->second;
        Material m{{.18f, .18f, .18f}, {0, 0, 0}, {0, 0, 0}, 0, .5f, 0, .01f, 0, nullptr, {}};
        TfToken uv_set;
        m.slot = keep(path.GetString());
        if (mat) {
            const UsdShadeShader shader = mat.ComputeSurfaceSource();
            TfToken id;
            require(shader && shader.GetIdAttr().Get(&id) && id == TfToken("UsdPreviewSurface"),
                    path.GetString() + ": expected a UsdPreviewSurface");
            copy3(map(shader, "diffuseColor", true, m.maps[0], uv_set)
                      ? GfVec3f(1)
                      : input(shader, "diffuseColor", GfVec3f(.18f)),
                  m.diffuse);
            copy3(input(shader, "emissiveColor", GfVec3f(0)), m.emissive);
            copy3(input(shader, "specularColor", GfVec3f(0)), m.specular);
            m.metallic = map(shader, "metallic", false, m.maps[3], uv_set)
                             ? 1.f
                             : input(shader, "metallic", 0.f);
            m.roughness = map(shader, "roughness", false, m.maps[2], uv_set)
                              ? 1.f
                              : input(shader, "roughness", .5f);
            m.clearcoat = input(shader, "clearcoat", 0.f);
            m.clearcoat_roughness = input(shader, "clearcoatRoughness", .01f);
            m.use_specular_workflow = input(shader, "useSpecularWorkflow", 0) != 0;
            require(input(shader, "opacity", 1.f) == 1.f,
                    path.GetString() + ": translucent material unsupported");
            require(input(shader, "displacement", 0.f) == 0.f,
                    path.GetString() + ": displacement unsupported");
            require((map(shader, "occlusion", false, m.maps[4], uv_set) ||
                     input(shader, "occlusion", 1.f) == 1.f) &&
                        input(shader, "ior", 1.5f) == 1.5f,
                    path.GetString() + ": custom occlusion or index of refraction unsupported");
            require(map(shader, "normal", true, m.maps[1], uv_set) ||
                        input(shader, "normal", GfVec3f(0, 0, 1)) == GfVec3f(0, 0, 1),
                    path.GetString() + ": unsupported constant tangent normal");
            for (auto x : {m.metallic, m.roughness, m.clearcoat, m.clearcoat_roughness})
                require(std::isfinite(x) && x >= 0 && x <= 1,
                        path.GetString() + ": invalid material value");
        }
        const auto index = static_cast<uint32_t>(materials.size());
        materials.push_back(m);
        material_uvs.push_back(uv_set);
        material_ids[path] = index;
        return index;
    }

    GfVec3d y_up(const GfVec3d &p) const { return z_up ? GfVec3d(p[0], p[2], -p[1]) : p; }

    int32_t part(const UsdGeomXformable &node) {
        Part part{};
        const auto prim = node.GetPrim();
        part.path = keep(prim.GetPath().GetString());
        const auto model = transforms.GetLocalToWorldTransform(prim) * relative_to_parent;
        for (int i = 0; i < 3; ++i) {
            GfVec3d axis(0);
            axis[i] = 1;
            const auto usd_axis = z_up ? GfVec3d(axis[0], -axis[2], axis[1]) : axis;
            copy3(y_up(model.TransformDir(usd_axis)), &part.frame[4 * i]);
        }
        const auto origin = y_up(model.Transform(GfVec3d(0))) * meters;
        copy3(origin, &part.frame[12]);
        part.frame[15] = 1;
        // XformCommonAPI stores its local rotation center in xformOp:translate:pivot.
        GfVec3d pivot(0);
        auto attr = prim.GetAttribute(TfToken("xformOp:translate:pivot"));
        if (attr && attr.HasAuthoredValueOpinion()) {
            GfVec3f f;
            if (!attr.Get(&pivot, time)) {
                require(attr.Get(&f, time),
                        prim.GetPath().GetString() + ": invalid authored pivot");
                pivot = GfVec3d(f);
            }
        }
        copy3(y_up(model.Transform(pivot)) * meters, part.pivot);
        part.parent = -1;
        if (!node.GetResetXformStack()) {
            for (auto ancestor = prim.GetParent(); ancestor; ancestor = ancestor.GetParent()) {
                auto parent = part_ids.find(ancestor.GetPath());
                if (parent != part_ids.end()) {
                    part.parent = parent->second;
                    break;
                }
            }
        }
        require(indices.size() <= UINT32_MAX, "too many rigid indices");
        part.first_index = uint32_t(indices.size());
        const auto id = static_cast<int32_t>(parts.size());
        parts.push_back(part);
        part_ids[prim.GetPath()] = id;
        return id;
    }

    static size_t element(const TfToken &mode, int vertex, size_t face, size_t corner,
                          const std::string &where) {
        if (mode == UsdGeomTokens->vertex || mode == UsdGeomTokens->varying)
            return vertex;
        if (mode == UsdGeomTokens->faceVarying)
            return corner;
        if (mode == UsdGeomTokens->constant)
            return 0;
        require(mode == UsdGeomTokens->uniform,
                where + ": unsupported primvar interpolation " + mode.GetString());
        return face;
    }

    void add(const UsdGeomMesh &mesh) {
        const std::string where = mesh.GetPath().GetString();
        for (auto prim = mesh.GetPrim(); prim; prim = prim.GetParent()) {
            const auto schemas = prim.GetAppliedSchemas();
            require(std::find(schemas.begin(), schemas.end(), TfToken("SkelBindingAPI")) ==
                        schemas.end(),
                    where + ": skeletal deformation and blend shapes unsupported");
        }
        TfToken scheme, orientation;
        mesh.GetSubdivisionSchemeAttr().Get(&scheme);
        mesh.GetOrientationAttr().Get(&orientation);
        require(scheme == UsdGeomTokens->none, where + ": subdivision surfaces unsupported");
        for (auto attr :
             {mesh.GetPointsAttr(), mesh.GetFaceVertexCountsAttr(), mesh.GetFaceVertexIndicesAttr(),
              mesh.GetNormalsAttr(), mesh.GetHoleIndicesAttr()})
            require(!attr.ValueMightBeTimeVarying(),
                    where + ": animated mesh geometry unsupported");
        VtVec3fArray points, normals;
        VtIntArray counts, corners, holes;
        require(mesh.GetPointsAttr().Get(&points) && mesh.GetFaceVertexCountsAttr().Get(&counts) &&
                    mesh.GetFaceVertexIndicesAttr().Get(&corners),
                where + ": missing mesh data");
        require(UsdGeomMesh::ValidateTopology(corners, counts, points.size(), nullptr),
                where + ": invalid topology");
        mesh.GetHoleIndicesAttr().Get(&holes);
        require(holes.empty(), where + ": mesh hole faces unsupported");
        const auto primvars = UsdGeomPrimvarsAPI(mesh.GetPrim());
        const auto normal_primvar = primvars.FindPrimvarWithInheritance(TfToken("normals"));
        TfToken interpolation;
        if (normal_primvar && normal_primvar.HasValue()) {
            require(!normal_primvar.GetAttr().ValueMightBeTimeVarying() &&
                        (!normal_primvar.IsIndexed() ||
                         !normal_primvar.GetIndicesAttr().ValueMightBeTimeVarying()) &&
                        normal_primvar.ComputeFlattened(&normals),
                    where + ": unsupported normal primvar");
            interpolation = normal_primvar.GetInterpolation();
        } else {
            mesh.GetNormalsAttr().Get(&normals);
            interpolation = mesh.GetNormalsInterpolation();
        }

        std::vector<GfVec3d> positions;
        const auto model = transforms.GetLocalToWorldTransform(mesh.GetPrim()) * relative_to_parent;
        require(std::abs(model.GetDeterminant()) > 1e-15, where + ": singular transform");
        const auto normal_transform = model.GetInverse().GetTranspose();
        const bool flip =
            (model.GetDeterminant() < 0) != (orientation == UsdGeomTokens->leftHanded);
        for (const auto &p : points)
            positions.push_back(y_up(model.Transform(GfVec3d(p))) * meters);

        UsdShadeMaterialBindingAPI binding(mesh);
        std::vector<int> face_material(counts.size(), -1);
        for (const auto &subset : binding.GetMaterialBindSubsets()) {
            TfToken element;
            subset.GetElementTypeAttr().Get(&element);
            VtIntArray faces;
            require(element == UsdGeomTokens->face &&
                        !subset.GetIndicesAttr().ValueMightBeTimeVarying() &&
                        subset.GetIndicesAttr().Get(&faces),
                    where + ": invalid face-material subset");
            const auto mat = UsdShadeMaterialBindingAPI(subset).ComputeBoundMaterial();
            require(bool(mat), subset.GetPath().GetString() + ": missing bound material");
            int index = static_cast<int>(material(mat));
            for (int i : faces) {
                require(i >= 0 && size_t(i) < counts.size() && face_material[i] == -1,
                        where + ": invalid or overlapping subset face");
                face_material[i] = index;
            }
        }
        if (std::find(face_material.begin(), face_material.end(), -1) != face_material.end()) {
            auto fallback = binding.ComputeBoundMaterial();
            const auto color = primvars.FindPrimvarWithInheritance(TfToken("displayColor"));
            require(fallback || !color || !color.GetAttr().HasAuthoredValueOpinion(),
                    where + ": unbound displayColor mesh unsupported");
            std::replace(face_material.begin(), face_material.end(), -1,
                         static_cast<int>(material(fallback)));
        }

        TfToken uv_set;
        for (int id : face_material) {
            const auto &name = material_uvs[id];
            if (!name.IsEmpty()) {
                require(uv_set.IsEmpty() || name == uv_set,
                        where + ": multiple texture-coordinate sets on one mesh unsupported");
                uv_set = name;
            }
        }
        VtVec2fArray uvs;
        const auto uv_primvar =
            primvars.FindPrimvarWithInheritance(uv_set.IsEmpty() ? TfToken("st") : uv_set);
        require(uv_set.IsEmpty() || (uv_primvar && uv_primvar.HasValue()),
                where + ": missing UV primvar " + uv_set.GetString());
        TfToken uv_interpolation;
        if (uv_primvar && uv_primvar.HasValue()) {
            require(!uv_primvar.GetAttr().ValueMightBeTimeVarying() &&
                        (!uv_primvar.IsIndexed() ||
                         !uv_primvar.GetIndicesAttr().ValueMightBeTimeVarying()) &&
                        uv_primvar.ComputeFlattened(&uvs) && !uvs.empty(),
                    where + ": invalid UV primvar");
            uv_interpolation = uv_primvar.GetInterpolation();
        }
        size_t start = 0;
        std::map<std::array<int, 4>, uint32_t> shared;
        for (size_t f = 0; f < counts.size(); ++f) {
            const int count = counts[f];
            require(count == 3 || count == 4,
                    where + ": only triangles and simple quads are supported");
            auto pos = [&](int c) { return positions[corners[start + c]]; };
            auto triangle_normal = [&](int a, int b, int c) {
                return GfCross(pos(b) - pos(a), pos(c) - pos(a));
            };
            int start_corner = 0;
            GfVec3d first = triangle_normal(0, 1, 2);
            // Prefer the original fan for convex quads. A simple concave quad
            // has only one diagonal whose two triangles have consistent winding.
            if (count == 4 && GfDot(first, triangle_normal(2, 3, 0)) <= 0) {
                start_corner = 1;
                first = triangle_normal(1, 2, 3);
                require(GfDot(first, triangle_normal(3, 0, 1)) > 0,
                        where + ": degenerate or crossing quad");
            }
            require(first.GetLength() > 1e-15, where + ": degenerate triangle");
            const GfVec3d flat = first.GetNormalized() * (flip ? -1.0 : 1.0);
            uint32_t face[4]{};
            for (int j = 0; j < count; ++j) {
                const int vertex = corners[start + j], mat = face_material[f];
                int ni = static_cast<int>(f);
                if (!normals.empty()) {
                    if (interpolation == UsdGeomTokens->vertex ||
                        interpolation == UsdGeomTokens->varying)
                        ni = vertex;
                    else if (interpolation == UsdGeomTokens->faceVarying)
                        ni = static_cast<int>(start + j);
                    else if (interpolation == UsdGeomTokens->constant)
                        ni = 0;
                    else
                        require(interpolation == UsdGeomTokens->uniform,
                                where + ": unsupported normal interpolation");
                    require(ni >= 0 && size_t(ni) < normals.size(),
                            where + ": normal index out of range");
                }
                const size_t ui =
                    uvs.empty() ? 0 : element(uv_interpolation, vertex, f, start + j, where);
                require(uvs.empty() || ui < uvs.size(), where + ": UV index out of range");
                const std::array<int, 4> key{vertex, ni, mat, static_cast<int>(ui)};
                auto found = shared.find(key);
                if (found == shared.end()) {
                    Vertex v{};
                    copy3(positions[vertex], v.position);
                    GfVec3d n = normals.empty()
                                    ? flat
                                    : y_up(normal_transform.TransformDir(GfVec3d(normals[ni])));
                    require(n.GetLength() > 1e-15, where + ": zero-length normal");
                    copy3(n.GetNormalized(), v.normal);
                    v.material = mat;
                    if (!uvs.empty()) {
                        const auto uv = uvs[ui];
                        require(std::isfinite(uv[0]) && std::isfinite(uv[1]),
                                where + ": nonfinite UV");
                        v.uv[0] = uv[0];
                        v.uv[1] = uv[1];
                    }
                    require(vertices.size() < UINT32_MAX, where + ": too many vertices");
                    found = shared.emplace(key, static_cast<uint32_t>(vertices.size())).first;
                    vertices.push_back(v);
                }
                face[j] = found->second;
            }
            for (int j = 1; j + 1 < count; ++j) {
                indices.push_back(face[start_corner]);
                indices.push_back(face[(start_corner + (flip ? j + 1 : j)) % count]);
                indices.push_back(face[(start_corner + (flip ? j : j + 1)) % count]);
            }
            start += count;
        }
    }
};
} // namespace

extern "C" int nothing_usd_mesh_load(const char *filename, const char *prim_path, double sample,
                                     uint32_t timed, void *user,
                                     void (*receive)(void *, const View *), char *error,
                                     size_t error_size) {
    try {
        TfErrorMark mark;
        auto stage = UsdStage::Open(filename);
        require(bool(stage), "unable to open stage");
        const SdfPath path(prim_path);
        require(path.IsAbsolutePath() && path.IsPrimPath(), "expected an absolute prim path");
        const auto root = stage->GetPrimAtPath(path);
        require(bool(root), std::string("prim does not exist: ") + prim_path);
        const auto up = UsdGeomGetStageUpAxis(stage);
        require(up == UsdGeomTokens->y || up == UsdGeomTokens->z, "unsupported stage up axis");
        Reader reader;
        reader.time = timed ? UsdTimeCode(sample) : UsdTimeCode::Default();
        reader.transforms.SetTime(reader.time);
        reader.meters = UsdGeomGetStageMetersPerUnit(stage);
        require(std::isfinite(reader.meters) && reader.meters > 0, "invalid stage metersPerUnit");
        reader.z_up = up == UsdGeomTokens->z;
        reader.relative_to_parent = reader.transforms.GetParentToWorldTransform(root).GetInverse();
        for (auto prim : UsdPrimRange(root, UsdTraverseInstanceProxies())) {
            const UsdGeomImageable imageable(prim);
            if (imageable) {
                require(!imageable.GetVisibilityAttr().ValueMightBeTimeVarying(),
                        prim.GetPath().GetString() + ": animated visibility unsupported");
                if (imageable.ComputeVisibility() == UsdGeomTokens->invisible)
                    continue;
                const auto purpose = imageable.ComputePurpose();
                if (purpose == UsdGeomTokens->guide || purpose == UsdGeomTokens->proxy)
                    continue;
            }
            const UsdGeomXformable xform(prim);
            const int part_id = xform ? reader.part(xform) : -1;
            const UsdGeomMesh mesh(prim);
            if (mesh) {
                reader.add(mesh);
                require(reader.indices.size() <= UINT32_MAX, "too many rigid indices");
                auto &part = reader.parts[part_id];
                part.index_count = uint32_t(reader.indices.size()) - part.first_index;
            } else
                require(!prim.IsA<UsdGeomGprim>() && !prim.IsA<UsdGeomPointInstancer>(),
                        prim.GetPath().GetString() + ": unsupported geometry in rigid mesh asset");
        }
        require(!reader.indices.empty(), "selected prim has no visible mesh triangles");
        if (!mark.IsClean()) {
            const auto message = mark.GetBegin()->GetCommentary();
            mark.Clear();
            throw ImportError{message};
        }
        const View view{reader.vertices.data(), reader.vertices.size(),  reader.indices.data(),
                        reader.indices.size(),  reader.materials.data(), reader.materials.size(),
                        reader.images.data(),   reader.images.size(),    reader.parts.data(),
                        reader.parts.size()};
        receive(user, &view);
        return 1;
    } catch (const ImportError &e) {
        std::snprintf(error, error_size, "%s", e.message.c_str());
        return 0;
    } catch (const std::exception &e) {
        std::snprintf(error, error_size, "%s", e.what());
        return 0;
    } catch (...) {
        std::snprintf(error, error_size, "OpenUSD import failed");
        return 0;
    }
}
