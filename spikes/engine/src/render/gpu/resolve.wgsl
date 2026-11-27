// Replace the accumulated premultiplied backdrop inside the finished mask.
// This pass uses no source-over blending, including on transparent groups.
struct Placement {
    roi: vec2<i32>,
    crop: vec2<i32>,
    mask_origin: vec2<f32>,
    inverse_x: vec2<f32>,
    inverse_y: vec2<f32>,
    phase: f32,
    visible_width: i32,
}
@group(0) @binding(0) var original: texture_2d<f32>;
@group(0) @binding(1) var filtered: texture_2d<f32>;
@group(0) @binding(2) var silhouette: texture_2d<f32>;
@group(0) @binding(3) var<uniform> place: Placement;

fn mask_size() -> vec2<f32> { return vec2<f32>(textureDimensions(silhouette)); }
fn mask_pixel(local: vec2<f32>) -> vec2<f32> {
    return local - vec2<f32>(place.crop);
}
fn coverage_tap(p: vec2<i32>) -> f32 {
    if any(p < vec2(0)) || p.x >= place.visible_width || p.y >= i32(textureDimensions(silhouette).y) {
        return 0.;
    }
    return textureLoad(silhouette, p, 0).a;
}
fn mask_alpha(local: vec2<f32>) -> f32 {
    let uv = mask_pixel(local) - vec2(0.5);
    let base = vec2<i32>(floor(uv));
    let f = fract(uv);
    let a = coverage_tap(base);
    if all(f == vec2(0.)) { return a; }
    return mix(mix(a, coverage_tap(base + vec2(1, 0)), f.x),
               mix(coverage_tap(base + vec2(0, 1)), coverage_tap(base + vec2(1)), f.x), f.y);
}
@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let screen = vec2<i32>(pixel.xy);
    let texel = screen - place.roi;
    let p = pixel.xy - place.mask_origin;
    let local = vec2(dot(p, place.inverse_x), dot(p, place.inverse_y));
    let alpha = mask_alpha(local);
    let m = clamp(coverage_amount(local, place.phase, alpha), 0.0, alpha);
    if m <= 0.0 { discard; }
    return mix(textureLoad(original, texel, 0), textureLoad(filtered, texel, 0), m);
}
