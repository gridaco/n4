/* C interface between the Rust crate and a pinned Cycles build.
 * All coordinates are right-handed, Y-up, meters, as in nothing-spatial.
 * Pixel buffers returned to the caller are top row first. */
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
#pragma GCC visibility push(default)

typedef struct N4Cycles N4Cycles;

typedef struct {
  float base_color[3];
  float metallic;
  float roughness;
  float specular_ior_level;
  float coat_weight;
  float coat_roughness;
  float emission[3];
  /* Image ids from n4c_image_create, or -1. */
  int32_t base_image;
  int32_t normal_image;
  int32_t roughness_image;
  int32_t roughness_channel; /* 0..3 */
  int32_t metallic_image;
  int32_t metallic_channel;
  /* Artwork on the surface. role 0 none, 1 base colour (composited over paint by alpha), 2 emission. */
  int32_t artwork_image;
  int32_t artwork_role;
  float emission_scale; /* radiance multiplier for role 2 as seen by the camera */
  float glow_scale;     /* extra multiplier on paths that do not start at the camera */
} N4Material;

/* device: "METAL" or "CPU". With `snapshots`, Cycles hands over in-progress pictures during a
 * render; it then denoises and reads back intermediate results, which cost about 0.6 s on a
 * 64-sample 1080p frame. Returns null on failure; n4c_last_error explains. */
N4Cycles *n4c_create(const char *device, int32_t threads, int32_t snapshots);
void n4c_destroy(N4Cycles *s);
const char *n4c_last_error(void);
const char *n4c_device_description(N4Cycles *s);

/* Images: rgba8 unassociated, top row first. srgb selects the colour interpretation. */
int32_t n4c_image_create(N4Cycles *s, int32_t width, int32_t height, int32_t srgb, const uint8_t *rgba);
/* Replace pixels (and optionally size) of an existing image; materials using it are refreshed. */
int32_t n4c_image_update(N4Cycles *s, int32_t image, int32_t width, int32_t height, const uint8_t *rgba);

int32_t n4c_material_create(N4Cycles *s, const N4Material *m);
int32_t n4c_material_update(N4Cycles *s, int32_t material, const N4Material *m);

/* One rigid object. positions/normals are per vertex (3 floats), uvs per vertex (2 floats, origin
 * lower left), indices per triangle corner, tri_material per triangle indexing `materials`.
 * pass_id > 0 makes the object addressable by masks. */
int32_t n4c_object_create(N4Cycles *s, const char *name, int32_t vertex_count, const float *positions,
                          const float *normals, const float *uvs, int32_t triangle_count,
                          const uint32_t *indices, const uint32_t *tri_material,
                          int32_t material_count, const int32_t *materials, int32_t pass_id);
/* Row-major 3x4 object-to-world. */
int32_t n4c_object_transform(N4Cycles *s, int32_t object, const float *m3x4);
int32_t n4c_object_camera_visible(N4Cycles *s, int32_t object, int32_t visible);

/* Rect light centred at `center`, spanning +-edge_u/2 and +-edge_v/2, emitting along cross(u, v).
 * `radiance` is the emitted scene-linear radiance (W sr^-1 m^-2 per channel). */
int32_t n4c_light_rect(N4Cycles *s, const float *center, const float *edge_u, const float *edge_v,
                       const float *radiance, int32_t camera_visible);
int32_t n4c_light_rect_update(N4Cycles *s, int32_t light, const float *center, const float *edge_u,
                              const float *edge_v, const float *radiance);
/* Distant light travelling along `direction`, with irradiance on a facing surface. */
int32_t n4c_light_distant(N4Cycles *s, const float *direction, const float *irradiance, float angle_degrees);
void n4c_background(N4Cycles *s, const float *radiance, int32_t transparent_film);

/* position, then right/up/forward unit axes of the view; fov_y in degrees. */
void n4c_camera(N4Cycles *s, const float *position, const float *right, const float *up,
                const float *forward, float fov_y_degrees, float near_clip);

typedef struct {
  int32_t width, height;
  int32_t samples;
  int32_t max_bounces;
  float clamp_indirect;
  int32_t denoise;
  int32_t seed;
  /* Object pass ids to produce visible-coverage masks for. */
  int32_t mask_count;
  const int32_t *mask_pass_ids;
  int32_t depth;       /* also capture camera distance per pixel */
  int32_t cryptomatte; /* also capture three Cryptomatte asset layers */
} N4Render;

typedef struct {
  double sync_seconds;   /* our scene edits + Session::reset */
  double render_seconds; /* start() to completion, includes Cycles scene update and denoise */
  double total_seconds;
  int32_t samples;
  int32_t cancelled;
} N4Timing;

/* Blocking render of one frame with the current scene. */
int32_t n4c_render(N4Cycles *s, const N4Render *r, N4Timing *timing);
/* rgba: width*height*4 floats, scene-linear, premultiplied, top row first. */
int32_t n4c_read_color(N4Cycles *s, float *rgba);
/* One float per pixel, coverage 0..1, for mask index i of the last render. */
int32_t n4c_read_mask(N4Cycles *s, int32_t i, float *coverage);
/* Camera-space depth, one float per pixel; not antialiased by Cycles. */
int32_t n4c_read_depth(N4Cycles *s, float *depth);
/* Cryptomatte asset layer 0..2: width*height*4 floats holding two (id, coverage) pairs per pixel. */
int32_t n4c_read_cryptomatte(N4Cycles *s, int32_t layer, float *rgba);
/* Safe to call from another thread while n4c_render blocks. */
void n4c_cancel(N4Cycles *s);
float n4c_progress(N4Cycles *s);
/* Number of in-progress pictures captured so far in the current render; n4c_read_color returns the latest. */
int32_t n4c_snapshot_count(N4Cycles *s);
/* Peak device memory in bytes, for receipts. */
int64_t n4c_peak_memory(N4Cycles *s);

#pragma GCC visibility pop
#ifdef __cplusplus
}
#endif
