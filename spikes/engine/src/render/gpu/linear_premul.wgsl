// Painter bytes associate coverage in encoded sRGB. Blend and filter in
// linear, premultiplied RGBA, including the antialiased edge of artwork.
fn n4_decode_srgb(encoded: vec3<f32>) -> vec3<f32> {
    return select(pow((encoded + 0.055) / 1.055, vec3(2.4)), encoded / 12.92, encoded <= vec3(0.04045));
}

fn n4_linear_premul(color: vec4<f32>) -> vec4<f32> {
    if color.a == 0.0 { return vec4(0.0); }
    let encoded = clamp(color.rgb / color.a, vec3(0.0), vec3(1.0));
    let linear = n4_decode_srgb(encoded);
    return vec4(linear * color.a, color.a);
}
