@group(0) @binding(0) var encoded: texture_2d<f32>;
@group(0) @binding(1) var<uniform> origin: vec2<i32>;

@fragment
fn fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return n4_linear_premul(textureLoad(encoded, origin + vec2<i32>(p.xy), 0));
}
