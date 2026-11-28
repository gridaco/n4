@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> source_color: u32;

@fragment
fn fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return n4_source_color(textureLoad(source, vec2<i32>(p.xy), 0), source_color);
}
