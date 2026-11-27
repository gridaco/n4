// Local pixel coordinates keep the grain stationary as the artwork moves or reveals.
fn effect(pixel: vec2<i32>, color: vec4<f32>, amount: f32) -> vec4<f32> {
    var seed = u32(pixel.x) * 0x8da6b343u ^ u32(pixel.y) * 0xd8163841u;
    seed ^= seed >> 15u;
    seed *= 0x7f4a7c15u;
    seed ^= seed >> 16u;
    let grain = f32(seed & 65535u) / 65535.0;
    return color * (1.0 - amount * smoothstep(0.42, 0.76, grain));
}
