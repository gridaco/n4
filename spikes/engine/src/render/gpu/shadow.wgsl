// Source-over tint in linear light. A sharp source is painted above this pass.
struct Tint {
    destination: vec2<i32>,
    _padding: vec2<i32>,
    color: vec4<f32>,
}
@group(0) @binding(0) var caster: texture_2d<f32>;
@group(0) @binding(1) var<uniform> tint: Tint;
@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let alpha = clamp(textureLoad(caster, vec2<i32>(pixel.xy) - tint.destination, 0).a, 0., 1.) * tint.color.a;
    return vec4(tint.color.rgb * alpha, alpha);
}
