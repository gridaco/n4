struct Material {
    rgb: u32,
    grain: u32,
    seed: u32,
    _padding: u32,
}
@group(0) @binding(0) var<uniform> plate: Material;

// Integer mixing keeps the noise field stable when revisiting a pixel or seed.
fn noise(p: vec2<u32>, seed: u32) -> f32 {
    var h = p.x * 0x9e3779b1u ^ p.y * 0x85ebca6bu ^ seed;
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return f32(h >> 16u) / 32767.5 - 1.0;
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let color = vec3<f32>(f32(plate.rgb & 255u), f32((plate.rgb >> 8u) & 255u),
                          f32((plate.rgb >> 16u) & 255u));
    var offset = 0.0;
    if plate.grain > 0u {
        let p = vec2<u32>(position.xy);
        let cell = p / 32u;
        let blend = smoothstep(vec2(0.0), vec2(1.0), fract(position.xy / 32.0));
        let seed = plate.seed ^ 0x5b57e66fu;
        let a = mix(noise(cell, seed), noise(cell + vec2(1u, 0u), seed), blend.x);
        let b = mix(noise(cell + vec2(0u, 1u), seed), noise(cell + vec2(1u), seed), blend.x);
        let fine = noise(p, plate.seed);
        let fibre = noise(vec2(p.x / 6u, p.y / 2u), plate.seed ^ 0xdf287ff1u);
        offset = f32(plate.grain) * (0.43 * fine + 0.15 * fibre + 0.42 * mix(a, b, blend.y));
    }
    // Author color and strength in sRGB code values. The sRGB target expects
    // linear fragment output and performs the final transfer on store.
    let s = clamp((color + offset) / 255.0, vec3(0.0), vec3(1.0));
    let linear = select(pow((s + 0.055) / 1.055, vec3(2.4)), s / 12.92, s <= vec3(0.04045));
    return vec4(linear, 1.0);
}
