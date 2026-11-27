struct N4HistoryWindow {
    // First component is the declared lookback. Missing pre-cut ages have empty slots.
    bounds: vec4<u32>,
    // Physical array layer for each logical age, starting with the current frame.
    slots: array<vec4<u32>, 4>,
}
@group(0) @binding(0) var n4_history_image: texture_2d_array<f32>;
@group(0) @binding(1) var<uniform> n4_window: N4HistoryWindow;

fn history_at(pixel: vec2<i32>, age: u32) -> vec4<f32> {
    if age > n4_window.bounds.x ||
       any(pixel < vec2<i32>(0)) || any(pixel >= vec2<i32>(textureDimensions(n4_history_image))) {
        return vec4<f32>(0.0);
    }
    let slot = n4_window.slots[age / 4u][age % 4u];
    if slot == 0xffffffffu {
        return vec4<f32>(0.0);
    }
    // Hardware decodes the sRGB source to straight linear color.
    let rgba = textureLoad(n4_history_image, pixel, i32(slot), 0);
    return vec4<f32>(rgba.rgb * rgba.a, rgba.a);
}

@fragment
fn fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(p.xy);
    return temporal(pixel, history_at(pixel, 0u), n4_window.bounds.x);
}
