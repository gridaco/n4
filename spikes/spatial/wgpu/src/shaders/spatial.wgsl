// Right-handed, Y-up world space in metres. Each axis is a matrix column.
struct View {
    eye: vec4<f32>,
    x: vec4<f32>,
    y: vec4<f32>,
    z: vec4<f32>,
    projection: vec4<f32>,
}
struct Pose {
    origin: vec4<f32>,
    x: vec4<f32>,
    y: vec4<f32>,
    z: vec4<f32>,
}
struct Material {
    diffuse_roughness: vec4<f32>,
    emission_metallic: vec4<f32>,
    specular_workflow: vec4<f32>,
    coat: vec4<f32>,
}
struct Light {
    direction: vec4<f32>,
    radiance: vec4<f32>,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> pose: Pose;
@group(0) @binding(2) var<storage, read> materials: array<Material>;
@group(0) @binding(3) var<storage, read> lights: array<Light>;
@group(0) @binding(4) var ink: texture_2d<f32>;
@group(0) @binding(5) var ink_sampler: sampler;
@group(0) @binding(6) var object_owners: texture_multisampled_2d<f32>;

fn projected(world: vec3<f32>) -> vec4<f32> {
    let offset = world - view.eye.xyz;
    // Reversed infinite perspective. Near distance maps to depth 1.
    return vec4(
        dot(offset, view.x.xyz) * view.projection.x,
        dot(offset, view.y.xyz) * view.projection.y,
        view.projection.z,
        -dot(offset, view.z.xyz),
    );
}

struct Surface {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) material: u32,
    @location(3) @interpolate(flat) owner: u32,
}
@vertex fn mesh_vertex(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) material: u32,
    @builtin(instance_index) owner: u32,
) -> Surface {
    let axes = mat3x3<f32>(pose.x.xyz, pose.y.xyz, pose.z.xyz);
    let world = pose.origin.xyz + axes * position;
    return Surface(projected(world), world, axes * normal, material, owner);
}

// GGX distribution, correlated Smith visibility, Schlick Fresnel.
fn specular_lobe(nh: f32, nv: f32, nl: f32, hv: f32, roughness: f32, f0: vec3<f32>) -> vec3<f32> {
    let alpha_squared = pow(max(roughness, 0.065), 4.);
    let d = nh * nh * (alpha_squared - 1.) + 1.;
    let distribution = alpha_squared / (3.14159 * d * d);
    let visibility = 0.5 / max(0.0001,
        nl * sqrt(nv * nv * (1. - alpha_squared) + alpha_squared) +
        nv * sqrt(nl * nl * (1. - alpha_squared) + alpha_squared));
    let fresnel = f0 + (1. - f0) * pow(1. - hv, 5.);
    return distribution * visibility * fresnel;
}

fn shaded_mesh(input: Surface) -> vec4<f32> {
    let material = materials[input.material];
    let normal = normalize(input.normal);
    let to_eye = normalize(view.eye.xyz - input.world);
    let nv = max(dot(normal, to_eye), 0.001);
    let base = material.diffuse_roughness.xyz;
    let specular_workflow = material.specular_workflow.w > 0.5;
    let metal = select(material.emission_metallic.w, 0., specular_workflow);
    let f0 = select(mix(vec3(0.04), base, metal), material.specular_workflow.xyz, specular_workflow);
    let diffuse = base * (1. - metal);
    var color = material.emission_metallic.xyz + diffuse * 0.08;
    for (var i = 0u; i < arrayLength(&lights); i++) {
        let light = lights[i];
        let nl = max(dot(normal, light.direction.xyz), 0.);
        if (nl > 0.) {
            let halfway = normalize(to_eye + light.direction.xyz);
            let nh = max(dot(normal, halfway), 0.);
            let hv = clamp(dot(halfway, to_eye), 0., 1.);
            let sheen = specular_lobe(nh, nv, nl, hv, material.diffuse_roughness.w, f0)
                + material.coat.x * specular_lobe(nh, nv, nl, hv, material.coat.y, vec3(0.04));
            color += light.radiance.xyz * nl * (diffuse / 3.14159 + sheen);
        }
    }
    return vec4(color, 1.);
}

struct Paper {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) owner: u32,
}
@vertex fn plane_vertex(@builtin(vertex_index) i: u32, @builtin(instance_index) owner: u32) -> Paper {
    let corners = array<vec2<f32>, 6>(
        vec2(0., 0.), vec2(1., 0.), vec2(0., 1.),
        vec2(0., 1.), vec2(1., 0.), vec2(1., 1.),
    );
    let uv = corners[i];
    let world = pose.origin.xyz + uv.x * pose.x.xyz + uv.y * pose.y.xyz;
    return Paper(projected(world), uv, owner);
}

struct InkOutput {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) mask: u32,
}
fn ink_samples(alpha: f32) -> u32 {
    // Same sample ownership in the beauty and matte passes, including glyph counters.
    let masks = array<u32, 5>(0u, 1u, 9u, 13u, 15u);
    return masks[u32(round(clamp(alpha, 0., 1.) * 4.))];
}

fn shaded_ink(input: Paper) -> InkOutput {
    let premul = textureSample(ink, ink_sampler, input.uv);
    let straight = clamp(premul.rgb / max(premul.a, 0.00001), vec3(0.), vec3(1.));
    let linear = select(pow((straight + 0.055) / 1.055, vec3(2.4)),
                        straight / 12.92, straight <= vec3(0.04045));
    return InkOutput(vec4(linear, 1.), ink_samples(premul.a));
}

@fragment fn mesh_fragment(input: Surface) -> @location(0) vec4<f32> {
    return shaded_mesh(input);
}
@fragment fn plane_fragment(input: Paper) -> InkOutput {
    return shaded_ink(input);
}

// Ownership is written by the same depth test that chose the displayed color.
struct TaggedSurface {
    @location(0) color: vec4<f32>,
    @location(1) owner: f32,
}
struct TaggedInk {
    @location(0) color: vec4<f32>,
    @location(1) owner: f32,
    @builtin(sample_mask) mask: u32,
}
@fragment fn tagged_mesh(input: Surface) -> TaggedSurface {
    return TaggedSurface(shaded_mesh(input), f32(input.owner) / 255.);
}
@fragment fn tagged_plane(input: Paper) -> TaggedInk {
    let painted = shaded_ink(input);
    return TaggedInk(painted.color, f32(input.owner) / 255., painted.mask);
}

struct Matte {
    @location(0) coverage: f32,
    @builtin(sample_mask) mask: u32,
}

fn matching_samples(pixel: vec2<i32>, owner: u32) -> u32 {
    var bits = 0u;
    for (var s = 0u; s < 4u; s++) {
        let stored = u32(round(textureLoad(object_owners, pixel, i32(s)).r * 255.));
        bits |= select(0u, 1u << s, stored == owner);
    }
    return bits;
}

@fragment fn projected_mesh() -> @location(0) f32 {
    return 1.;
}
@fragment fn projected_plane(input: Paper) -> Matte {
    return Matte(1., ink_samples(textureSample(ink, ink_sampler, input.uv).a));
}
@fragment fn owned_mesh(input: Surface) -> Matte {
    return Matte(1., matching_samples(vec2<i32>(input.clip.xy), input.owner));
}
@fragment fn owned_plane(input: Paper) -> Matte {
    let covered = ink_samples(textureSample(ink, ink_sampler, input.uv).a);
    return Matte(1., covered & matching_samples(vec2<i32>(input.clip.xy), input.owner));
}
