@group(0) @binding(0) var artwork: texture_2d<f32>;
@group(0) @binding(1) var<uniform> layer: Layer;
@group(0) @binding(2) var coverage: texture_2d<f32>;

struct Layer {
    origin: vec2<i32>,
    artwork_origin: vec2<i32>,
    mask_origin: vec2<i32>,
    opacity: f32,
    amount: f32,
    encoded_premul: u32,
    masked: u32,
    transformed: u32,
    wipe_right_local: i32,
    inverse_x: vec2<f32>,
    inverse_y: vec2<f32>,
    destination_origin: vec2<f32>,
    destination_art: u32,
    snapshot_origin: vec2<i32>,
    blur_params: vec4<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(corners[index], 0.0, 1.0);
}

fn n4_source(dst: vec2<i32>, texel: vec2<i32>) -> vec4<f32> {
    var color = textureLoad(artwork, texel, 0);
    if layer.encoded_premul != 0u {
        color = n4_linear_premul(color);
    }
    color = n4_grade(n4_blur_mix(dst - layer.origin, texel, color));
    var opacity = layer.opacity;
    if layer.masked != 0u {
        opacity *= textureLoad(coverage, dst - layer.mask_origin, 0).a;
    }
    return effect(dst - layer.origin, color, layer.amount) * opacity;
}

// Each tap sees drawable-local effects before resampling. Transparency beyond
// the artwork and mask must be explicit on backends that clamp texture loads.
fn n4_tap(local: vec2<i32>, screen: vec2<i32>) -> vec4<f32> {
    let texel = select(local + layer.origin, screen, layer.destination_art != 0u) - layer.artwork_origin;
    if local.x >= layer.wipe_right_local || any(texel < vec2(0)) ||
        any(texel >= vec2<i32>(textureDimensions(artwork))) { return vec4(0.0); }
    if layer.masked != 0u {
        let mask = local - (layer.mask_origin - layer.origin);
        if any(mask < vec2(0)) || any(mask >= vec2<i32>(textureDimensions(coverage))) {
            return vec4(0.0);
        }
    }
    return n4_source(local + layer.origin, texel);
}

fn n4_pixel(pixel: vec4<f32>) -> vec4<f32> {
    let dst = vec2<i32>(pixel.xy);
    if layer.transformed == 0u {
        // The integer translation fast path uses a single source texel.
        return n4_source(dst, dst - layer.artwork_origin);
    }
    let p = pixel.xy - layer.destination_origin;
    let q = vec2(dot(p, layer.inverse_x), dot(p, layer.inverse_y)) - vec2(0.5);
    let base = vec2<i32>(floor(q));
    let f = fract(q);
    let row0 = mix(n4_tap(base, dst), n4_tap(base + vec2(1, 0), dst), f.x);
    let row1 = mix(n4_tap(base + vec2(0, 1), dst), n4_tap(base + vec2(1, 1), dst), f.x);
    return mix(row0, row1, f.y);
}
