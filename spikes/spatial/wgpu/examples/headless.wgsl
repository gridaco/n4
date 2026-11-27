// The host composites the spatial result over magenta. A yellow overprint
// identifies the part of the green/blue card hidden behind the red foil.
@group(0) @binding(0) var beauty: texture_2d<f32>;
@group(0) @binding(1) var hidden: texture_2d<f32>;

@vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array(vec2(-1., -1.), vec2(3., -1.), vec2(-1., 3.));
    return vec4(p[i], 0., 1.);
}
@fragment fn fragment(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    let scene = textureLoad(beauty, xy, 0);
    let overprint = textureLoad(hidden, xy, 0).r;
    let paper = vec3(1., 0., 1.);
    return vec4(scene.rgb + vec3(0., overprint, 0.) + (1. - scene.a) * paper, 1.);
}
