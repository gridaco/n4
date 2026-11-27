struct Screen {
    @builtin(position) p: vec4f, @location(0) uv: vec2f
}

@vertex fn fullscreen(@builtin(vertex_index) i: u32) -> Screen {
    let p = vec2f(f32((i<<1u)&2u), f32(i&2u));
    return Screen(vec4f(2. * p.x - 1., 1. - 2. * p.y, 0., 1.), p);
}
@group(0) @binding(0) var original: texture_2d<f32>;
@group(0) @binding(1) var filter_sampler: sampler;
@fragment fn downsample(in: Screen) -> @location(0) vec4f {
    return textureSampleLevel(original, filter_sampler, in.uv, 0.);
}
