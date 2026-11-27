// Standard RGB grade. The working image is linear and premultiplied; the
// authored transfer is on straight, normalized sRGB and leaves coverage intact.
struct RgbGrade {
    gain: vec3<f32>,
    offset: vec3<f32>,
}
@group(0) @binding(5) var<storage, read> n4_grades: array<RgbGrade>;

fn n4_grade(color: vec4<f32>) -> vec4<f32> {
    if color.a <= 0.0 { return vec4(0.0); }
    let linear = clamp(color.rgb / color.a, vec3(0.0), vec3(1.0));
    var rgb = select(1.055 * pow(linear, vec3(1.0 / 2.4)) - 0.055,
                     12.92 * linear, linear <= vec3(0.0031308));
    // Adjacent grades share transfer conversions, while saturating at the
    // end of each authored operation just as two separate passes would.
    for (var i = 0u; i < arrayLength(&n4_grades); i++) {
        rgb = clamp(rgb * n4_grades[i].gain + n4_grades[i].offset, vec3(0.0), vec3(1.0));
    }
    return vec4(n4_decode_srgb(rgb) * color.a, color.a);
}
