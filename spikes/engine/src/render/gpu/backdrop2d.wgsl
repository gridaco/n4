// The compositor samples the already-painted scope before replacing any of its pixels.
struct BackdropWindow {
    roi_origin: vec2<u32>,
    radius: u32,
    unused: u32,
    mask_size: vec2<u32>,
    padding: vec2<u32>,
    mask_origin: vec2<f32>,
    crop: vec2<f32>,
    inverse_x: vec2<f32>,
    inverse_y: vec2<f32>,
    params: vec4<f32>,
}
@group(0) @binding(0) var backdrop_image: texture_2d<f32>;
@group(0) @binding(1) var<uniform> window: BackdropWindow;
var<private> backdrop_screen: vec2<f32>;

fn backdrop_params() -> vec4<f32> { return window.params; }

fn backdrop_screen_delta(local_offset: vec2<f32>) -> vec2<f32> {
    let ix = window.inverse_x;
    let iy = window.inverse_y;
    let det = ix.x * iy.y - ix.y * iy.x;
    return vec2f(iy.y * local_offset.x - ix.y * local_offset.y,
                 ix.x * local_offset.y - iy.x * local_offset.x) / det;
}

// Offsets are in the mask's local pixels. Transform them into scope pixels and
// limit each output-space axis to the declared support. Use the nearest edge
// of the compositing scope when sampling beyond the frame.
fn backdrop_at(local_offset: vec2<f32>) -> vec4<f32> {
    let delta = backdrop_screen_delta(local_offset);
    let r = f32(window.radius);
    let hi = vec2i(textureDimensions(backdrop_image)) - 1;
    let src = clamp(backdrop_screen - 0.5 + clamp(delta, vec2f(-r), vec2f(r)),
                    vec2f(0.0), vec2f(hi));
    let base = vec2i(floor(src));
    let next = min(base + 1, hi);
    let t = fract(src);
    return mix(mix(textureLoad(backdrop_image, base, 0),
                   textureLoad(backdrop_image, vec2i(next.x, base.y), 0), t.x),
               mix(textureLoad(backdrop_image, vec2i(base.x, next.y), 0),
                   textureLoad(backdrop_image, next, 0), t.x), t.y);
}

@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    backdrop_screen = vec2f(window.roi_origin) + pixel.xy;
    let p = backdrop_screen - window.mask_origin;
    let local = vec2f(dot(p, window.inverse_x), dot(p, window.inverse_y)) - window.crop;
    return backdrop_pixel(local, vec2f(window.mask_size), backdrop_screen);
}
