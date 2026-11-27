// Coordinates are in the current compositing scope. Only the directional
// support around the output pixel is available to a sampled fixture filter.
struct Sampling {
    input_origin: vec2<i32>,
    input_size: vec2<i32>,
    output_origin: vec2<i32>,
    direction: vec2<i32>,
    radius: i32,
    transparent_edges: u32,
    _pad1: vec2<i32>,
    weights: array<vec4<f32>, 25>,
}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> sampling: Sampling;
var<private> current_pixel: vec2<i32>;

fn support() -> i32 { return sampling.radius; }
fn weight(offset: i32) -> f32 {
    let index = u32(min(abs(offset), sampling.radius));
    return sampling.weights[index / 4u][index % 4u];
}
fn tap(p: vec2<i32>, offset: i32) -> vec4<f32> {
    let delta = dot(p - current_pixel, sampling.direction) + offset;
    let pos = current_pixel + sampling.direction * clamp(delta, -sampling.radius, sampling.radius);
    let relative = pos - sampling.input_origin;
    if sampling.transparent_edges != 0u && (any(relative < vec2(0)) || any(relative >= sampling.input_size)) {
        return vec4(0.);
    }
    let texel = clamp(pos - sampling.input_origin, vec2(0), sampling.input_size - vec2(1));
    return textureLoad(source, texel, 0);
}
@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    current_pixel = sampling.output_origin + vec2<i32>(pixel.xy);
    if sampling.radius == 0 { return tap(current_pixel, 0); }
    return filter_pixel(current_pixel);
}
