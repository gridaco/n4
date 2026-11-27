@group(0) @binding(0) var samples: texture_2d_array<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec2<f32>, 3>(vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0));
    return vec4(corners[index], 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    var sum = vec4(0.0);
    let count = textureNumLayers(samples);
    for (var i = 0u; i < count; i++) {
        // sRGB array sampling returns linear-light RGB for each finished subframe.
        sum += textureLoad(samples, vec2<i32>(at.xy), i32(i), 0);
    }
    return sum / f32(count);
}
