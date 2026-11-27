@group(0) @binding(4) var n4_blurred: texture_2d<f32>;

// Evaluate the effect before coverage, blending and spatial resampling.
// A missing focus band applies the effect uniformly to the source.
fn n4_blur_mix(local: vec2<i32>, texel: vec2<i32>, sharp: vec4<f32>) -> vec4<f32> {
    let params = layer.blur_params;
    var coverage = params.x;
    if params.w > 0.0 {
        let distance = abs(f32(local.y) + 0.5 - params.y);
        let t = clamp((distance - params.z) / params.w, 0.0, 1.0);
        coverage *= t * t * (3.0 - 2.0 * t);
    }
    return mix(sharp, textureLoad(n4_blurred, texel, 0), coverage);
}
