// The compositor schedules this same one-dimensional kernel along X and Y.
// Each weight comes from the normalized, finite Gaussian for this sigma.
fn filter_pixel(p: vec2<i32>) -> vec4<f32> {
    let center = tap(p, 0);
    // Accumulating differences preserves every constant channel exactly. In
    // particular, an opaque source stays opaque in both RGBA16F intermediates.
    var delta = vec4(0.0);
    for (var i = 1; i <= support(); i++) {
        delta += ((tap(p, -i) - center) + (tap(p, i) - center)) * weight(i);
    }
    return center + delta;
}
