// Resolve the four scene-linear samples first. Apply exposure and a continuous
// highlight shoulder once to straight color, then premultiply for the 2D stack.
struct Screen {
    @builtin(position) p: vec4f
}

@vertex fn fullscreen(@builtin(vertex_index) i: u32) -> Screen {
    let p = vec2f(f32((i<<1u)&2u), f32(i&2u));
    return Screen(vec4f(2. * p.x - 1., 1. - 2. * p.y, 0., 1.));
}
@group(0) @binding(0) var linear_scene: texture_2d<f32>;
@group(0) @binding(1) var<uniform> exposure: vec4f;
@fragment fn display(s: Screen) -> @location(0) vec4f {
    let p = textureLoad(linear_scene, vec2i(s.p.xy), 0);
    let c = max(vec3f(0.), p.rgb / max(p.a, 1e-7) * exposure.x);
    let excess = max(c - .7, vec3f(0.));
    let mapped = min(c, vec3f(.7)) + .3 * excess / (.3 + excess);
    return vec4f(mapped * p.a, p.a);
}
