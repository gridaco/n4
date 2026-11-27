// The matte grid shares the resolved scene's top-left origin and pixel scale.
@group(0) @binding(6) var n4_object_mattes: texture_2d_array<f32>;

fn n4_matte_at(p: vec2<i32>, index: i32) -> f32 {
    let size = vec2<i32>(textureDimensions(n4_object_mattes, 0));
    if any(p < vec2<i32>(0)) || any(p >= size) { return 0.; }
    return textureLoad(n4_object_mattes, p, index, 0).r;
}
