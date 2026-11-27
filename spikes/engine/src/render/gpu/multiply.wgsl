@group(0) @binding(3) var n4_backdrop: texture_2d<f32>;

@fragment
fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let s = n4_pixel(pixel);
    let d = textureLoad(n4_backdrop, vec2<i32>(pixel.xy) - layer.snapshot_origin, 0);
    // Source-over with Multiply over their overlap, in premultiplied linear RGB.
    return vec4(s.rgb * (1.0 - d.a) + d.rgb * (1.0 - s.a) + s.rgb * d.rgb,
                s.a + d.a * (1.0 - s.a));
}
