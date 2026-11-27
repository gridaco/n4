/* Bridge from nothing-spatial scene data to a long-lived Cycles session.
 * Written against Cycles v5.3.0. The Cycles C++ API carries no stability promise. */
#include "n4_cycles.h"

#include "device/device.h"
/* shader.h must precede background.h, or Background::set_shader binds to the bool overload. */
#include "scene/shader.h"
#include "scene/background.h"
#include "scene/camera.h"
#include "scene/film.h"
#include "scene/image.h"
#include "scene/image_loader.h"
#include "scene/integrator.h"
#include "scene/light.h"
#include "scene/mesh.h"
#include "scene/object.h"
#include "scene/pass.h"
#include "scene/scene.h"
#include "scene/shader_graph.h"
#include "scene/shader_nodes.h"
#include "session/buffers.h"
#include "session/output_driver.h"
#include "session/session.h"
#include "util/image_metadata.h"
#include "util/log.h"
#include "util/path.h"
#include "util/time.h"
#include "util/transform.h"

#include <dlfcn.h>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

using namespace ccl;

static std::string g_error;

namespace {

struct ImageData {
  int width = 0, height = 0;
  bool srgb = true;
  uint64_t revision = 0;
  std::shared_ptr<std::vector<uint8_t>> pixels;
};

/* Serves caller-owned RGBA8 pixels to Cycles. Cycles stores images bottom row first. */
class MemoryLoader : public ImageLoader {
 public:
  MemoryLoader(const int id, const ImageData &data) : id(id), data(data) {}

  /* Pixels are decoded to scene-linear floats here, so the result does not depend on whether an
   * OpenColorIO configuration is present. The image manager finalizes the metadata afterwards. */
  bool load_metadata(ImageMetaData &metadata, const ImageLoaderParams &, Progress &) override
  {
    metadata.width = data.width;
    metadata.height = data.height;
    metadata.channels = 4;
    metadata.type = IMAGE_DATA_TYPE_FLOAT4;
    metadata.colorspace = data.srgb ? u_colorspace_scene_linear : u_colorspace_data;
    return true;
  }

  bool load_pixels(const ImageMetaData &metadata, void *pixels) override
  {
    static float lut[256];
    static std::once_flag once;
    std::call_once(once, [] {
      for (int i = 0; i < 256; i++) {
        const float x = i / 255.0f;
        lut[i] = x <= 0.04045f ? x / 12.92f : powf((x + 0.055f) / 1.055f, 2.4f);
      }
    });
    float *out = static_cast<float *>(pixels);
    const uint8_t *in = data.pixels->data();
    for (int y = 0; y < data.height; y++) {
      const uint8_t *row = in + size_t(data.width) * 4 * (data.height - 1 - y);
      float *o = out + size_t(data.width) * 4 * y;
      for (int x = 0; x < data.width * 4; x += 4) {
        for (int c = 0; c < 3; c++) {
          o[x + c] = data.srgb ? lut[row[x + c]] : row[x + c] / 255.0f;
        }
        o[x + 3] = row[x + 3] / 255.0f;
      }
    }
    metadata.conform_pixels(pixels);
    return true;
  }

  string name() const override
  {
    return "n4_image_" + std::to_string(id) + "_" + std::to_string(data.revision);
  }

  bool equals(const ImageLoader &other) const override
  {
    const MemoryLoader *o = dynamic_cast<const MemoryLoader *>(&other);
    return o && o->id == id && o->data.revision == data.revision;
  }

  int id;
  ImageData data;
};

struct Captured {
  std::mutex mutex;
  int width = 0, height = 0;
  int snapshots = 0;
  std::vector<float> color;
  std::vector<float> depth;
  std::vector<std::vector<float>> masks;
  std::vector<std::vector<float>> crypto;
};

class CaptureDriver : public OutputDriver {
 public:
  CaptureDriver(Captured *out, int mask_count, bool depth, bool crypto)
      : out(out), mask_count(mask_count), depth(depth), crypto(crypto)
  {
  }

  static void flip(std::vector<float> &dst, const std::vector<float> &src, int w, int h, int c)
  {
    dst.resize(src.size());
    for (int y = 0; y < h; y++) {
      memcpy(&dst[size_t(y) * w * c], &src[size_t(h - 1 - y) * w * c], sizeof(float) * w * c);
    }
  }

  bool capture(const Tile &tile, bool final)
  {
    if (!(tile.size == tile.full_size)) {
      return false;
    }
    const int w = tile.size.x, h = tile.size.y;
    std::vector<float> tmp(size_t(w) * h * 4);
    if (!tile.get_pass_pixels("combined", 4, tmp.data())) {
      return false;
    }
    const std::lock_guard<std::mutex> lock(out->mutex);
    out->width = w;
    out->height = h;
    flip(out->color, tmp, w, h, 4);
    if (!final) {
      out->snapshots++;
      return true;
    }
    out->masks.assign(mask_count, std::vector<float>());
    std::vector<float> one(size_t(w) * h);
    for (int i = 0; i < mask_count; i++) {
      if (tile.get_pass_pixels("n4_mask_" + std::to_string(i), 1, one.data())) {
        flip(out->masks[i], one, w, h, 1);
      }
    }
    out->depth.clear();
    if (depth && tile.get_pass_pixels("depth", 1, one.data())) {
      flip(out->depth, one, w, h, 1);
    }
    out->crypto.clear();
    if (crypto) {
      for (int i = 0; i < 3; i++) {
        if (tile.get_pass_pixels("CryptoAsset0" + std::to_string(i), 4, tmp.data())) {
          out->crypto.emplace_back();
          flip(out->crypto.back(), tmp, w, h, 4);
        }
      }
    }
    return true;
  }

  void write_render_tile(const Tile &tile) override
  {
    capture(tile, true);
  }

  bool update_render_tile(const Tile &tile) override
  {
    return capture(tile, false);
  }

  Captured *out;
  int mask_count;
  bool depth, crypto;
};

struct MaterialState {
  Shader *shader = nullptr;
  N4Material desc;
};

}  // namespace

struct N4Cycles {
  std::unique_ptr<Session> session;
  SessionParams params;
  std::string device_description;
  std::vector<ImageData> images;
  std::vector<MaterialState> materials;
  std::vector<Object *> objects;
  std::vector<Object *> lights;
  std::vector<int> mask_ids; /* pass ids baked into the current shader graphs */
  Captured captured;
  bool passes_dirty = true;
  int last_denoise = -1;
  int last_outputs = -1;
};

static Transform tfm_from_rows(const float *m)
{
  return make_transform(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11]);
}

static ImageHandle make_handle(N4Cycles *s, int id)
{
  ImageParams params;
  params.interpolation = INTERPOLATION_LINEAR;
  params.extension = EXTENSION_REPEAT;
  params.alpha_type = IMAGE_ALPHA_UNASSOCIATED;
  params.colorspace = s->images[id].srgb ? u_colorspace_scene_linear : u_colorspace_data;
  return s->session->scene->image_manager->add_image(
      make_unique<MemoryLoader>(id, s->images[id]), params, false);
}

static ImageTextureNode *texture(N4Cycles *s, ShaderGraph *graph, int id)
{
  ImageTextureNode *node = graph->create_node<ImageTextureNode>();
  node->handle = make_handle(s, id);
  node->set_colorspace(s->images[id].srgb ? u_colorspace_scene_linear : u_colorspace_data);
  node->set_alpha_type(IMAGE_ALPHA_UNASSOCIATED);
  node->set_extension(EXTENSION_REPEAT);
  return node;
}

static ShaderOutput *channel(ShaderGraph *graph, ImageTextureNode *tex, int ch)
{
  if (ch == 3) {
    return tex->output("Alpha");
  }
  SeparateColorNode *sep = graph->create_node<SeparateColorNode>();
  graph->connect(tex->output("Color"), sep->input("Color"));
  return sep->output(ch == 0 ? "Red" : ch == 1 ? "Green" : "Blue");
}

/* Build the surface graph for one material. Every graph also writes one coverage AOV per
 * requested mask, so visible masks are filtered exactly like the beauty. */
static void build_graph(N4Cycles *s, MaterialState &m)
{
  const N4Material &d = m.desc;
  unique_ptr<ShaderGraph> graph = make_unique<ShaderGraph>();
  ShaderGraph *g = graph.get();

  PrincipledBsdfNode *bsdf = g->create_node<PrincipledBsdfNode>();
  bsdf->set_base_color(make_float3(d.base_color[0], d.base_color[1], d.base_color[2]));
  bsdf->set_metallic(d.metallic);
  bsdf->set_roughness(d.roughness);
  bsdf->set_specular_ior_level(d.specular_ior_level);
  bsdf->set_coat_weight(d.coat_weight);
  bsdf->set_coat_roughness(d.coat_roughness);
  bsdf->set_emission_color(make_float3(d.emission[0], d.emission[1], d.emission[2]));
  bsdf->set_emission_strength((d.emission[0] + d.emission[1] + d.emission[2]) > 0.0f ? 1.0f : 0.0f);

  ShaderOutput *base = nullptr;
  if (d.base_image >= 0) {
    base = texture(s, g, d.base_image)->output("Color");
  }
  ImageTextureNode *art = (d.artwork_image >= 0 && d.artwork_role != 0) ?
                              texture(s, g, d.artwork_image) :
                              nullptr;
  if (art && d.artwork_role == 1) {
    /* Artwork over paint, by the artwork's own coverage. */
    MixNode *mix = g->create_node<MixNode>();
    mix->set_mix_type(NODE_MIX_BLEND);
    mix->set_color1(make_float3(d.base_color[0], d.base_color[1], d.base_color[2]));
    if (base) {
      g->connect(base, mix->input("Color1"));
    }
    g->connect(art->output("Color"), mix->input("Color2"));
    g->connect(art->output("Alpha"), mix->input("Fac"));
    base = mix->output("Color");
  }
  if (base) {
    g->connect(base, bsdf->input("Base Color"));
  }
  if (d.roughness_image >= 0) {
    g->connect(channel(g, texture(s, g, d.roughness_image), d.roughness_channel), bsdf->input("Roughness"));
  }
  if (d.metallic_image >= 0) {
    g->connect(channel(g, texture(s, g, d.metallic_image), d.metallic_channel), bsdf->input("Metallic"));
  }
  if (d.normal_image >= 0) {
    NormalMapNode *nm = g->create_node<NormalMapNode>();
    g->connect(texture(s, g, d.normal_image)->output("Color"), nm->input("Color"));
    g->connect(nm->output("Normal"), bsdf->input("Normal"));
  }

  ShaderOutput *surface = bsdf->output("BSDF");
  if (art && d.artwork_role == 2) {
    /* A screen: emits the artwork. Camera rays see emission_scale; other paths see it times glow. */
    EmissionNode *em = g->create_node<EmissionNode>();
    g->connect(art->output("Color"), em->input("Color"));
    LightPathNode *lp = g->create_node<LightPathNode>();
    MixNode *gain = g->create_node<MixNode>();
    gain->set_mix_type(NODE_MIX_BLEND);
    gain->set_color1(make_float3(d.emission_scale * d.glow_scale));
    gain->set_color2(make_float3(d.emission_scale));
    g->connect(lp->output("Is Camera Ray"), gain->input("Fac"));
    RGBToBWNode *bw = g->create_node<RGBToBWNode>();
    g->connect(gain->output("Color"), bw->input("Color"));
    g->connect(bw->output("Val"), em->input("Strength"));
    surface = em->output("Emission");
  }
  g->connect(surface, g->output()->input("Surface"));

  if (!s->mask_ids.empty()) {
    ObjectInfoNode *info = g->create_node<ObjectInfoNode>();
    for (size_t i = 0; i < s->mask_ids.size(); i++) {
      MathNode *cmp = g->create_node<MathNode>();
      cmp->set_math_type(NODE_MATH_COMPARE);
      cmp->set_value2(float(s->mask_ids[i]));
      cmp->set_value3(0.5f);
      g->connect(info->output("Object Index"), cmp->input("Value1"));
      OutputAOVNode *aov = g->create_node<OutputAOVNode>();
      aov->set_name(ustring("n4_mask_" + std::to_string(i)));
      g->connect(cmp->output("Value"), aov->input("Value"));
    }
  }

  m.shader->set_graph(std::move(graph));
  m.shader->tag_update(s->session->scene.get());
}

extern "C" {

const char *n4c_last_error(void)
{
  return g_error.c_str();
}

N4Cycles *n4c_create(const char *device, int32_t threads)
{
  static std::once_flag once;
  std::call_once(once, [] {
    log_init(nullptr);
    /* Cycles compiles its Metal kernels from source at run time and looks for that source
     * relative to the executable. Point it at the directory holding this library instead. */
    Dl_info info;
    std::string dir;
    if (dladdr((void *)&n4c_last_error, &info) && info.dli_fname) {
      dir = path_dirname(info.dli_fname);
    }
    if (const char *env = getenv("N4_CYCLES_RESOURCES")) {
      dir = env;
    }
    path_init(dir, "");
  });
  try {
    auto s = std::make_unique<N4Cycles>();
    const DeviceType type = Device::type_from_string(device);
    bool found = false;
    for (const DeviceInfo &info : Device::available_devices(DEVICE_MASK(type))) {
      if (info.meets_driver_requirement) {
        s->params.device = info;
        s->device_description = info.description;
        found = true;
        break;
      }
    }
    if (!found) {
      g_error = std::string("Cycles device not available: ") + device;
      return nullptr;
    }
    /* N4_CYCLES_INTERACTIVE=1 selects Cycles' viewport scheduling: frequent in-progress pictures. */
    s->params.background = getenv("N4_CYCLES_INTERACTIVE") == nullptr;
    /* Not headless: Cycles then hands in-progress pictures to the output driver during a render. */
    s->params.headless = false;
    s->params.threads = threads;
    s->params.use_auto_tile = false;
    s->params.tile_size = 0;
    s->params.samples = 1;
    SceneParams scene_params;
    scene_params.shadingsystem = SHADINGSYSTEM_SVM;
    s->session = make_unique<Session>(s->params, scene_params);
    return s.release();
  }
  catch (const std::exception &e) {
    g_error = e.what();
    return nullptr;
  }
}

void n4c_destroy(N4Cycles *s)
{
  if (s) {
    s->session->cancel(true);
    delete s;
  }
}

const char *n4c_device_description(N4Cycles *s)
{
  return s->device_description.c_str();
}

int32_t n4c_image_create(N4Cycles *s, int32_t width, int32_t height, int32_t srgb, const uint8_t *rgba)
{
  ImageData d;
  d.width = width;
  d.height = height;
  d.srgb = srgb != 0;
  d.pixels = std::make_shared<std::vector<uint8_t>>(rgba, rgba + size_t(width) * height * 4);
  s->images.push_back(d);
  return int32_t(s->images.size() - 1);
}

int32_t n4c_image_update(N4Cycles *s, int32_t image, int32_t width, int32_t height, const uint8_t *rgba)
{
  const thread_scoped_lock lock(s->session->scene->mutex);
  ImageData &d = s->images[image];
  d.width = width;
  d.height = height;
  d.revision++;
  d.pixels = std::make_shared<std::vector<uint8_t>>(rgba, rgba + size_t(width) * height * 4);
  for (MaterialState &m : s->materials) {
    const N4Material &x = m.desc;
    if (x.base_image == image || x.normal_image == image || x.roughness_image == image ||
        x.metallic_image == image || x.artwork_image == image)
    {
      build_graph(s, m);
    }
  }
  return 0;
}

int32_t n4c_material_create(N4Cycles *s, const N4Material *m)
{
  const thread_scoped_lock lock(s->session->scene->mutex);
  MaterialState state;
  state.shader = s->session->scene->create_node<Shader>();
  state.desc = *m;
  s->materials.push_back(state);
  build_graph(s, s->materials.back());
  return int32_t(s->materials.size() - 1);
}

int32_t n4c_material_update(N4Cycles *s, int32_t material, const N4Material *m)
{
  const thread_scoped_lock lock(s->session->scene->mutex);
  s->materials[material].desc = *m;
  build_graph(s, s->materials[material]);
  return 0;
}

int32_t n4c_object_create(N4Cycles *s, const char *name, int32_t vertex_count, const float *positions,
                          const float *normals, const float *uvs, int32_t triangle_count,
                          const uint32_t *indices, const uint32_t *tri_material,
                          int32_t material_count, const int32_t *materials, int32_t pass_id)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Mesh *mesh = scene->create_node<Mesh>();
  mesh->name = ustring(name);
  array<Node *> used;
  for (int i = 0; i < material_count; i++) {
    used.push_back_slow(s->materials[materials[i]].shader);
  }
  mesh->set_used_shaders(used);
  mesh->resize_mesh(vertex_count, triangle_count);
  packed_float3 *P = mesh->get_position_for_write();
  for (int i = 0; i < vertex_count; i++) {
    P[i] = packed_float3(make_float3(positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]));
  }
  int *tris = mesh->get_triangles().data();
  for (int i = 0; i < triangle_count * 3; i++) {
    tris[i] = int(indices[i]);
  }
  int *shader = mesh->get_shader().data();
  bool *smooth = mesh->get_smooth().data();
  for (int i = 0; i < triangle_count; i++) {
    shader[i] = int(tri_material[i]);
    smooth[i] = true;
  }
  mesh->tag_triangles_modified();
  mesh->tag_shader_modified();
  mesh->tag_smooth_modified();
  if (normals) {
    Attribute *attr = mesh->attributes.add(ATTR_STD_VERTEX_NORMAL);
    packed_normal *N = attr->data_for_write<packed_normal>();
    for (int i = 0; i < vertex_count; i++) {
      N[i] = packed_normal(make_float3(normals[i * 3], normals[i * 3 + 1], normals[i * 3 + 2]));
    }
  }
  if (uvs) {
    Attribute *attr = mesh->attributes.add(ATTR_STD_UV);
    float2 *uv = attr->data_for_write<float2>();
    for (int i = 0; i < triangle_count * 3; i++) {
      const uint32_t v = indices[i];
      uv[i] = make_float2(uvs[v * 2], uvs[v * 2 + 1]);
    }
  }
  Object *object = scene->create_node<Object>();
  object->name = ustring(name);
  object->set_geometry(mesh);
  object->set_tfm(transform_identity());
  object->set_pass_id(pass_id);
  object->set_asset_name(ustring("n4_asset_" + std::to_string(pass_id)));
  object->set_visibility(PATH_RAY_VISIBILITY_ALL);
  object->tag_update(scene);
  s->objects.push_back(object);
  return int32_t(s->objects.size() - 1);
}

int32_t n4c_object_transform(N4Cycles *s, int32_t object, const float *m)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Object *o = s->objects[object];
  o->set_tfm(tfm_from_rows(m));
  o->tag_update(scene);
  return 0;
}

int32_t n4c_object_camera_visible(N4Cycles *s, int32_t object, int32_t visible)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Object *o = s->objects[object];
  const uint v = o->get_visibility();
  o->set_visibility(visible ? (v | PATH_RAY_VISIBILITY_CAMERA) : (v & ~PATH_RAY_VISIBILITY_CAMERA));
  o->tag_update(scene);
  return 0;
}

static void place_rect(Object *object, AreaLight *light, const float *c, const float *u, const float *v,
                       const float *radiance)
{
  const float3 eu = make_float3(u[0], u[1], u[2]), ev = make_float3(v[0], v[1], v[2]);
  const float lu = len(eu), lv = len(ev);
  const float3 x = eu / lu, y = ev / lv;
  /* Cycles emits along -Z of the light's transform. */
  const float3 z = -normalize(cross(x, y));
  object->set_tfm(make_transform(x.x, y.x, z.x, c[0], x.y, y.y, z.y, c[1], x.z, y.z, z.z, c[2]));
  light->set_sizeu(lu);
  light->set_sizev(lv);
  /* With normalize off, emitted radiance is strength / pi (checked by `spike calib`). */
  light->set_normalize(false);
  light->set_strength(make_float3(radiance[0], radiance[1], radiance[2]) * M_PI_F);
}

int32_t n4c_light_rect(N4Cycles *s, const float *center, const float *edge_u, const float *edge_v,
                       const float *radiance, int32_t camera_visible)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Object *object = scene->create_node<Object>();
  AreaLight *light = scene->create_node<AreaLight>();
  light->set_ellipse(false);
  light->set_use_mis(true);
  object->set_geometry(light);
  object->set_visibility(camera_visible ? PATH_RAY_VISIBILITY_ALL :
                                          (PATH_RAY_VISIBILITY_ALL & ~PATH_RAY_VISIBILITY_CAMERA));
  Shader *shader = scene->create_node<Shader>();
  unique_ptr<ShaderGraph> graph = make_unique<ShaderGraph>();
  EmissionNode *em = graph->create_node<EmissionNode>();
  em->set_color(one_float3());
  em->set_strength(1.0f);
  graph->connect(em->output("Emission"), graph->output()->input("Surface"));
  shader->set_graph(std::move(graph));
  shader->tag_update(scene);
  array<Node *> used;
  used.push_back_slow(shader);
  light->set_used_shaders(used);
  place_rect(object, light, center, edge_u, edge_v, radiance);
  light->tag_update(scene);
  object->tag_update(scene);
  s->lights.push_back(object);
  return int32_t(s->lights.size() - 1);
}

int32_t n4c_light_rect_update(N4Cycles *s, int32_t light, const float *center, const float *edge_u,
                              const float *edge_v, const float *radiance)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Object *object = s->lights[light];
  AreaLight *l = static_cast<AreaLight *>(object->get_geometry());
  place_rect(object, l, center, edge_u, edge_v, radiance);
  l->tag_update(scene);
  object->tag_update(scene);
  return 0;
}

int32_t n4c_light_distant(N4Cycles *s, const float *direction, const float *irradiance, float angle_degrees)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Object *object = scene->create_node<Object>();
  SunLight *light = scene->create_node<SunLight>();
  light->set_use_mis(true);
  light->set_angle(angle_degrees * M_PI_F / 180.0f);
  light->set_strength(make_float3(irradiance[0], irradiance[1], irradiance[2]));
  object->set_geometry(light);
  object->set_visibility(PATH_RAY_VISIBILITY_ALL & ~PATH_RAY_VISIBILITY_CAMERA);
  Shader *shader = scene->create_node<Shader>();
  unique_ptr<ShaderGraph> graph = make_unique<ShaderGraph>();
  EmissionNode *em = graph->create_node<EmissionNode>();
  em->set_color(one_float3());
  em->set_strength(1.0f);
  graph->connect(em->output("Emission"), graph->output()->input("Surface"));
  shader->set_graph(std::move(graph));
  shader->tag_update(scene);
  array<Node *> used;
  used.push_back_slow(shader);
  light->set_used_shaders(used);
  /* Light travels along `direction`; Cycles' sun shines along -Z of its transform. */
  const float3 d = normalize(make_float3(direction[0], direction[1], direction[2]));
  const float3 zc = -d;
  float3 xc = cross(fabs(zc.y) < 0.99f ? make_float3(0, 1, 0) : make_float3(1, 0, 0), zc);
  xc = normalize(xc);
  const float3 yc = cross(zc, xc);
  object->set_tfm(make_transform(xc.x, yc.x, zc.x, 0, xc.y, yc.y, zc.y, 0, xc.z, yc.z, zc.z, 0));
  light->tag_update(scene);
  object->tag_update(scene);
  s->lights.push_back(object);
  return int32_t(s->lights.size() - 1);
}

void n4c_background(N4Cycles *s, const float *radiance, int32_t transparent_film)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  unique_ptr<ShaderGraph> graph = make_unique<ShaderGraph>();
  BackgroundNode *bg = graph->create_node<BackgroundNode>();
  bg->set_color(make_float3(radiance[0], radiance[1], radiance[2]));
  bg->set_strength(1.0f);
  graph->connect(bg->output("Background"), graph->output()->input("Surface"));
  scene->default_background->set_graph(std::move(graph));
  scene->default_background->tag_update(scene);
  scene->background->set_shader(scene->default_background);
  scene->background->set_transparent(transparent_film != 0);
  scene->background->tag_update(scene);
}

void n4c_camera(N4Cycles *s, const float *p, const float *r, const float *u, const float *f,
                float fov_y_degrees, float near_clip)
{
  Scene *scene = s->session->scene.get();
  const thread_scoped_lock lock(scene->mutex);
  Camera *cam = scene->camera;
  /* Cycles camera space: +X right, +Y up, +Z forward. */
  cam->set_matrix(make_transform(r[0], u[0], f[0], p[0], r[1], u[1], f[1], p[1], r[2], u[2], f[2], p[2]));
  cam->set_camera_type(CAMERA_PERSPECTIVE);
  cam->set_fov(fov_y_degrees * M_PI_F / 180.0f);
  cam->set_nearclip(near_clip);
  cam->set_farclip(1000.0f);
  cam->need_flags_update = true;
  cam->need_device_update = true;
}

int32_t n4c_render(N4Cycles *s, const N4Render *r, N4Timing *timing)
{
  try {
    Session *session = s->session.get();
    Scene *scene = session->scene.get();
    const double t0 = time_dt();
    {
      const thread_scoped_lock lock(scene->mutex);
      std::vector<int> ids(r->mask_pass_ids, r->mask_pass_ids + r->mask_count);
      if (ids != s->mask_ids) {
        s->mask_ids = ids;
        for (MaterialState &m : s->materials) {
          build_graph(s, m);
        }
        s->passes_dirty = true;
      }
      const int outputs = (r->depth ? 1 : 0) | (r->cryptomatte ? 2 : 0);
      if (s->passes_dirty || s->last_denoise != r->denoise || s->last_outputs != outputs) {
        const vector<Pass *> &passes = scene->passes;
        scene->delete_nodes(set<Pass *>(passes.begin(), passes.end()));
        Pass *combined = scene->create_node<Pass>();
        combined->set_name(ustring("combined"));
        combined->set_type(PASS_COMBINED);
        combined->set_mode(r->denoise ? PassMode::DENOISED : PassMode::NOISY);
        for (size_t i = 0; i < s->mask_ids.size(); i++) {
          Pass *aov = scene->create_node<Pass>();
          aov->set_name(ustring("n4_mask_" + std::to_string(i)));
          aov->set_type(PASS_AOV_VALUE);
        }
        if (r->depth) {
          Pass *depth = scene->create_node<Pass>();
          depth->set_name(ustring("depth"));
          depth->set_type(PASS_DEPTH);
        }
        scene->film->set_cryptomatte_passes(r->cryptomatte ? CRYPT_ASSET : CRYPT_NONE);
        scene->film->set_cryptomatte_depth(r->cryptomatte ? 3 : 0);
        for (int i = 0; r->cryptomatte && i < 3; i++) {
          Pass *crypto = scene->create_node<Pass>();
          crypto->set_name(ustring("CryptoAsset0" + std::to_string(i)));
          crypto->set_type(PASS_CRYPTOMATTE);
        }
        s->passes_dirty = false;
        s->last_denoise = r->denoise;
        s->last_outputs = outputs;
      }
      Integrator *in = scene->integrator;
      in->set_max_bounce(r->max_bounces);
      in->set_max_diffuse_bounce(r->max_bounces);
      in->set_max_glossy_bounce(r->max_bounces);
      in->set_sample_clamp_indirect(r->clamp_indirect);
      in->set_seed(r->seed);
      in->set_use_adaptive_sampling(false);
      in->set_use_denoise(r->denoise != 0);
      in->set_denoiser_type(DENOISER_OPENIMAGEDENOISE);
      scene->camera->set_full_width(r->width);
      scene->camera->set_full_height(r->height);
      scene->camera->compute_auto_viewplane();
      scene->camera->need_flags_update = true;
      scene->camera->need_device_update = true;

      s->params.samples = r->samples;
      BufferParams buffer;
      buffer.width = r->width;
      buffer.height = r->height;
      buffer.full_width = r->width;
      buffer.full_height = r->height;
      session->progress.reset();
      {
        const std::lock_guard<std::mutex> capture_lock(s->captured.mutex);
        s->captured.snapshots = 0;
      }
      session->set_output_driver(
          make_unique<CaptureDriver>(&s->captured, r->mask_count, r->depth != 0, r->cryptomatte != 0));
      session->reset(s->params, buffer);
    }
    const double t1 = time_dt();
    session->start();
    session->wait();
    const double t2 = time_dt();
    if (timing) {
      timing->sync_seconds = t1 - t0;
      timing->render_seconds = t2 - t1;
      timing->total_seconds = t2 - t0;
      timing->samples = r->samples;
      timing->cancelled = session->progress.get_cancel() ? 1 : 0;
    }
    if (session->progress.get_error()) {
      g_error = session->progress.get_error_message();
      return -1;
    }
    return 0;
  }
  catch (const std::exception &e) {
    g_error = e.what();
    return -1;
  }
}

int32_t n4c_read_color(N4Cycles *s, float *rgba)
{
  const std::lock_guard<std::mutex> lock(s->captured.mutex);
  if (s->captured.color.empty()) {
    g_error = "no colour captured";
    return -1;
  }
  memcpy(rgba, s->captured.color.data(), s->captured.color.size() * sizeof(float));
  return 0;
}

int32_t n4c_read_mask(N4Cycles *s, int32_t i, float *coverage)
{
  if (i < 0 || size_t(i) >= s->captured.masks.size() || s->captured.masks[i].empty()) {
    g_error = "mask not captured";
    return -1;
  }
  memcpy(coverage, s->captured.masks[i].data(), s->captured.masks[i].size() * sizeof(float));
  return 0;
}

int32_t n4c_read_depth(N4Cycles *s, float *depth)
{
  if (s->captured.depth.empty()) {
    g_error = "depth not captured";
    return -1;
  }
  memcpy(depth, s->captured.depth.data(), s->captured.depth.size() * sizeof(float));
  return 0;
}

int32_t n4c_read_cryptomatte(N4Cycles *s, int32_t layer, float *rgba)
{
  if (layer < 0 || size_t(layer) >= s->captured.crypto.size()) {
    g_error = "cryptomatte layer not captured";
    return -1;
  }
  memcpy(rgba, s->captured.crypto[layer].data(), s->captured.crypto[layer].size() * sizeof(float));
  return 0;
}

void n4c_cancel(N4Cycles *s)
{
  s->session->cancel(true);
}

float n4c_progress(N4Cycles *s)
{
  return s->session->get_progress();
}

int32_t n4c_snapshot_count(N4Cycles *s)
{
  const std::lock_guard<std::mutex> lock(s->captured.mutex);
  return s->captured.snapshots;
}

int64_t n4c_peak_memory(N4Cycles *s)
{
  return int64_t(s->session->device->stats.mem_peak);
}

}  // extern "C"
