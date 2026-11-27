// Linear Rec.709 working colors. Texture views perform the authored transfer decoding.
struct Scene {
    eye: vec4f, x: vec4f, y: vec4f, z: vec4f, projection: vec4f,
    shadow_center_extent: vec4f, shadow_x_bias: vec4f, shadow_y_radius: vec4f, shadow_z_enabled: vec4f,
    environment: vec4f,
}

struct Light {
    direction: vec4f, radiance: vec4f
}

struct Pose {
    origin: vec4f, x: vec4f, y: vec4f, z: vec4f, artwork: vec4f
}

struct Reader {
    flags: vec4f, scale: vec4f, bias: vec4f
}

struct Material {
    base_roughness: vec4f, emission_metal: vec4f, specular_workflow: vec4f, coat: vec4f, readers: array<Reader, 5>
}
@group(0) @binding(0) var<uniform> scene: Scene;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_comparison: sampler_comparison;
@group(0) @binding(4) var owners: texture_multisampled_2d<f32>;
// rig[0] is a header: center = ambient radiance and rect count, u.x = whether rect 0 is shadowed.
struct Rect {
    center: vec4f, u: vec4f, v: vec4f, radiance: vec4f
}
@group(0) @binding(5) var<storage, read> rig: array<Rect>;
@group(1) @binding(0) var<uniform> pose: Pose;
@group(1) @binding(1) var<uniform> material: Material;
@group(1) @binding(2) var base_image: texture_2d<f32>;
@group(1) @binding(3) var normal_image: texture_2d<f32>;
@group(1) @binding(4) var roughness_image: texture_2d<f32>;
@group(1) @binding(5) var metallic_image: texture_2d<f32>;
@group(1) @binding(6) var occlusion_image: texture_2d<f32>;
@group(1) @binding(7) var artwork: texture_2d<f32>;
@group(1) @binding(8) var map_sampler: sampler;
@group(1) @binding(9) var art_sampler: sampler;

struct Vertex {
    @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) material_index: u32, @location(3) st: vec2f,
    @builtin(instance_index) owner: u32,
}

struct Surface {
    @builtin(position) clip: vec4f, @location(0) world: vec3f, @location(1) normal: vec3f,
    @location(2) st: vec2f, @location(3) @interpolate(flat) owner: u32,
}

fn shadow_position(p: vec3f) -> vec3f {
    let delta = (p - scene.shadow_center_extent.xyz) / scene.shadow_center_extent.w;
    return vec3f(dot(delta, scene.shadow_x_bias.xyz), dot(delta, scene.shadow_y_radius.xyz), .5 + .25 * dot(delta, scene.shadow_z_enabled.xyz));
}

fn surface(v: Vertex) -> Surface {
    let axes = mat3x3f(pose.x.xyz, pose.y.xyz, pose.z.xyz);
    let p = pose.origin.xyz + axes * v.position;
    let offset = p - scene.eye.xyz;
    let clip = vec4f(dot(offset, scene.x.xyz) * scene.projection.x, dot(offset, scene.y.xyz) * scene.projection.y, scene.projection.z, -dot(offset, scene.z.xyz));
    return Surface(clip, p, axes * v.normal, v.st, v.owner);
}

@vertex fn vertex(v: Vertex) -> Surface {
    return surface(v);
}

@vertex fn shadow_vertex(v: Vertex) -> Surface {
    var s = surface(v);
    s.clip = vec4f(shadow_position(s.world), 1.);
    return s;
}

fn uv(s: Surface) -> vec2f {
    return vec2f(s.st.x, 1. - s.st.y);
}

fn coverage(s: Surface) -> f32 {
    if (pose.artwork.x == 2. || pose.artwork.x == 3.) {
        return textureSample(artwork, art_sampler, uv(s)).a;
    }
    return 1.;
}

fn bits(alpha: f32) -> u32 {
    return array<u32, 5>(0u, 1u, 9u, 13u, 15u)[u32(round(clamp(alpha, 0., 1.) * 4.))];
}

@fragment fn depth_only(s: Surface) {
    if (coverage(s)<.5) {
        discard;
    }
}

fn shadow_visibility(p: vec3f, geometric_normal: vec3f, nl: f32) -> f32 {
    let projected = shadow_position(p);
    let uv = vec2f(projected.x * .5 + .5, .5 - projected.y * .5);
    if (scene.shadow_z_enabled.w < .5 || any(uv < vec2f(0.)) || any(uv > vec2f(1.)) ||
        projected.z < 0. || projected.z > 1.) {
        return 1.;
    }
    let texel = 1. / vec2f(textureDimensions(shadow_map));
    let extent = scene.shadow_center_extent.w;
    let radius = scene.shadow_y_radius.w;
    let nd = dot(geometric_normal, scene.shadow_z_enabled.xyz);
    // Receiver-plane depth at each PCF tap. A flat floor seen obliquely by the
    // key has different light-space depths at adjacent texels. Comparing every
    // tap to the center depth darkens the entire shadow camera's footprint.
    var gradient = vec2f(0.);
    if (abs(nd) > .0001) {
        gradient = .5 * vec2f(-dot(geometric_normal, scene.shadow_x_bias.xyz),
            dot(geometric_normal, scene.shadow_y_radius.xyz)) / nd;
    }
    // Hardware bilinear PCF still covers four neighboring depth samples.
    let filter_bias = dot(abs(gradient), texel);
    let bias = scene.shadow_x_bias.w * (1. + 2. * (1. - nl)) / (4. * extent) + filter_bias;
    var visibility = 0.;
    var weight = 0.;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let offset = vec2f(f32(x), f32(y)) * .5 * radius * texel;
            let tap = uv + offset;
            let depth = projected.z + dot(gradient, offset) - bias;
            let w = f32((3 - abs(x)) * (3 - abs(y)));
            if (all(tap >= vec2f(0.)) && all(tap <= vec2f(1.))) {
                visibility += w * textureSampleCompareLevel(shadow_map, shadow_comparison, tap, depth);
            }
            else {
                visibility += w;
            }
            weight += w;
        }
    }
    return visibility / weight;
}

fn read_color(image: texture_2d<f32>, coords: vec2f, r: Reader) -> vec4f {
    let v = textureSample(image, map_sampler, coords) * r.scale + r.bias;
    switch (u32(r.flags.y)) {
        case 1u: {
            return vec4f(v.r);
        }
        case 2u: {
            return vec4f(v.g);
        }
        case 3u: {
            return vec4f(v.b);
        }
        case 4u: {
            return vec4f(v.a);
        }
        default: {
            return v;
        }
    }
}

fn unit(v: vec3f, fallback: vec3f) -> vec3f {
    return select(fallback, v * inverseSqrt(max(dot(v, v), 1e-12)), dot(v, v)>1e-10);
}

fn square(x: f32) -> f32 {
    return x * x;
}

fn environment(reflected: vec3f, rough: f32) -> vec3f {
    let c = scene.environment.x;
    let s = scene.environment.y;
    let d = vec3f(c * reflected.x + s * reflected.z, reflected.y, -s * reflected.x + c * reflected.z);
    // Two long studio strip boxes and a broad overhead bounce. Convolving the
    // angular width with roughness broadens and attenuates the reflected strips.
    let spread = rough * rough;
    let key = exp(-square((d.x - .45) / (.12 + spread * .75)) - square((d.y - .48) / (.70 + spread * .4))) * smoothstep(-.15, .30, d.z) / (1. + 5. * spread);
    let rim = exp(-square((d.y - .20) / (.10 + spread * .7)) - square((d.x + .45) / (.72 + spread * .3))) * smoothstep(-.25, .18, -d.z) / (1. + 4. * spread);
    return (vec3f(.05, .06, .075) + vec3f(.30, .34, .42) * smoothstep(-.4, 1., d.y)+
        vec3f(3.8, 3.35, 2.82) * key + vec3f(2., 2.75, 3.6) * rim) * scene.environment.z;
}

fn specular(nh: f32, nv: f32, nl: f32, hv: f32, r: f32, f0: vec3f) -> vec3f {
    let aa = pow(max(r, .065), 4.);
    let d = nh * nh * (aa - 1.) + 1.;
    let g = .5 / max(.0001, nl * sqrt(nv * nv * (1. - aa) + aa) + nv * sqrt(nl * nl * (1. - aa) + aa));
    return aa / (3.14159 * d * d) * g * (f0 + (1. - f0) * pow(1. - hv, 5.));
}

// One-sided rect light. Diffuse uses the exact unoccluded polygon form factor. Specular uses the
// point on the rectangle nearest the mirror direction, with the lobe widened by the light's size.
// Neither accounts for occlusion; the caller applies the shadow map to one light.
fn rect_light(r: Rect, p: vec3f, n: vec3f, eye: vec3f, nv: f32, diffuse: vec3f, f0: vec3f, rough: f32, coat: vec2f, cap: vec3f) -> vec3f {
    let facing = normalize(cross(r.u.xyz, r.v.xyz));
    let c = r.center.xyz - p;
    if (dot(c, facing) >= 0.) {
        return vec3f(0.);
    }
    let hu = .5 * r.u.xyz;
    let hv = .5 * r.v.xyz;
    let q = array<vec3f, 4>(normalize(c - hu - hv), normalize(c + hu - hv), normalize(c + hu + hv), normalize(c - hu + hv));
    var vector = vec3f(0.);
    for (var i = 0u;i<4u;i++) {
        let a = q[i];
        let b = q[(i + 1u) % 4u];
        let x = cross(a, b);
        let len = length(x);
        if (len>1e-6) {
            vector += acos(clamp(dot(a, b), -1., 1.)) * x / len;
        }
    }
    vector *= select(-1., 1., dot(vector, c) >= 0.);
    let form = max(dot(vector, n), 0.) / 6.28318;
    var value = diffuse * form;

    let lu = length(hu);
    let lv = length(hv);
    let du = hu / lu;
    let dv = hv / lv;
    let mirror = reflect(-eye, n);
    let denom = dot(mirror, facing);
    var hit = mirror * length(c);
    if (denom < -1e-4) {
        hit = mirror * (dot(c, facing) / denom);
    }
    let rel = hit - c;
    let point = c + du * clamp(dot(rel, du), -lu, lu) + dv * clamp(dot(rel, dv), -lv, lv);
    let d2 = max(dot(point, point), 1e-8);
    let l = point * inverseSqrt(d2);
    let nl = max(dot(n, l), 0.);
    if (nl>0.) {
        let area = 4. * lu * lv;
        let solid = min(area * max(dot(-l, facing), 0.) / d2, 6.28318);
        let h = unit(eye + l, n);
        let nh = max(dot(n, h), 0.);
        let hv2 = clamp(dot(h, eye), 0., 1.);
        // Representative-point area light: evaluate the surface's own lobe toward the nearest point
        // of the rectangle, and scale by (a / a')^2 so the peak drops as the source grows.
        let spread = sqrt(area) / (3. * sqrt(d2));
        let a0 = max(rough * rough, .0042);
        let norm = square(a0 / clamp(a0 + spread, 0., 1.));
        let c0 = max(coat.y * coat.y, .0042);
        let coat_norm = square(c0 / clamp(c0 + spread, 0., 1.));
        // A light that fills the whole lobe reflects at most the surface's reflectance, as the
        // ambient term does.
        let lobe = solid * nl * (norm * specular(nh, nv, nl, hv2, rough, f0) + coat_norm * coat.x * specular(nh, nv, nl, hv2, coat.y, vec3f(.04)));
        value += min(lobe, cap);
    }
    return r.radiance.xyz * value;
}

struct Ink {
    @location(0) color: vec4f, @builtin(sample_mask) mask: u32
}

fn shade(s: Surface) -> Ink {
    let coords = uv(s);
    var n = unit(s.normal, vec3f(0., 1., 0.));
    // Derivatives use authored USD st, where increasing t points up.
    let dx = dpdx(s.world);
    let dy = dpdy(s.world);
    let tx = dpdx(s.st);
    let ty = dpdy(s.st);
    let det = tx.x * ty.y - tx.y * ty.x;
    if (material.readers[1].flags.x>.5 && abs(det)>1e-9) {
        let du = (dx * ty.y - dy * tx.y) / det;
        let dv = (dy * tx.x - dx * ty.x) / det;
        let t = unit(du - n * dot(n, du), vec3f(1., 0., 0.));
        let b = unit(cross(n, t), vec3f(0., 1., 0.)) * select(-1., 1., dot(cross(n, t), dv)>=0.);
        let mapped = read_color(normal_image, coords, material.readers[1]).xyz;
        n = unit(mat3x3f(t, b, n) * mapped, n);
    }
    var base = material.base_roughness.xyz;
    if (material.readers[0].flags.x>.5) {
        base *= read_color(base_image, coords, material.readers[0]).rgb;
    }
    if (pose.artwork.x>.5) {
        let color = textureSample(artwork, art_sampler, coords);
        var straight = color.rgb / max(color.a, 1e-7);
        // Standalone graphic planes use the existing raw premultiplied sRGB texture contract.
        if (pose.artwork.x == 3.) {
            straight = select(pow((straight + .055) / 1.055, vec3f(2.4)), straight / 12.92, straight<=vec3f(.04045));
        }
        if (pose.artwork.x>=2.) {
            return Ink(vec4f(straight, 1.), bits(color.a));
        }
        base = mix(base, straight, color.a);
    }
    var rough = material.base_roughness.w;
    if (material.readers[2].flags.x>.5) {
        rough *= read_color(roughness_image, coords, material.readers[2]).x;
    }
    var metal = material.emission_metal.w;
    if (material.readers[3].flags.x>.5) {
        metal *= read_color(metallic_image, coords, material.readers[3]).x;
    }
    var ao = 1.;
    if (material.readers[4].flags.x>.5) {
        ao = read_color(occlusion_image, coords, material.readers[4]).x;
    }
    rough = clamp(rough, .04, 1.);
    metal = clamp(metal, 0., 1.);
    let spec = material.specular_workflow.w>.5;
    metal = select(metal, 0., spec);
    let f0 = select(mix(vec3f(.04), base, metal), material.specular_workflow.xyz, spec);
    let diffuse = base * (1. - metal);
    let eye = unit(scene.eye.xyz - s.world, n);
    let nv = max(dot(n, eye), .001);
    let reflected = reflect(-eye, n);
    let fresnel = f0 + (max(vec3f(1. - rough), f0) - f0) * pow(1. - nv, 5.);
    var value = material.emission_metal.xyz;
    if (scene.environment.w>.5) {
        // A rig replaces the analytic studio: constant ambient radiance, then each rect light.
        value += ao * rig[0].center.xyz * (diffuse + fresnel);
        let count = u32(rig[0].center.w);
        for (var i = 0u;i<count;i++) {
            var visibility = 1.;
            if (i == 0u && rig[0].u.x>.5) {
                let toward = unit(rig[1].center.xyz - s.world, n);
                visibility = shadow_visibility(s.world, unit(cross(dx, dy), n), max(dot(n, toward), 0.));
            }
            value += visibility * rect_light(rig[i + 1u], s.world, n, eye, nv, diffuse, f0, rough, material.coat.xy, fresnel);
        }
    }
    else {
        value += ao * (diffuse * (.07 + scene.environment.z * (.12 + .17 * max(n.y, 0.))) + fresnel * environment(reflected, rough));
    }
    for (var i = 0u;i<arrayLength(&lights);i++) {
        let l = lights[i];
        let nl = max(dot(n, l.direction.xyz), 0.);
        if (nl>0.) {
            let h = unit(eye + l.direction.xyz, n);
            let nh = max(dot(n, h), 0.);
            let hv = clamp(dot(h, eye), 0., 1.);
            var visibility = 1.;
            if (i == 0u) {
                visibility = shadow_visibility(s.world, unit(cross(dx, dy), n), nl);
            }
            let brdf = specular(nh, nv, nl, hv, rough, f0) + material.coat.x * specular(nh, nv, nl, hv, material.coat.y, vec3f(.04));
            value += visibility * l.radiance.xyz * nl * (diffuse / 3.14159 + brdf);
        }
    }
    return Ink(vec4f(max(value, vec3f(0.)), 1.), 15u);
}

@fragment fn beauty(s: Surface) -> Ink {
    return shade(s);
}

struct Tagged {
    @location(0) color: vec4f, @location(1) owner: f32, @builtin(sample_mask) mask: u32
}

@fragment fn tagged(s: Surface) -> Tagged {
    let c = shade(s);
    return Tagged(c.color, f32(s.owner) / 255., c.mask);
}

struct Matte {
    @location(0) v: f32, @builtin(sample_mask) mask: u32
}

@fragment fn projected(s: Surface) -> Matte {
    return Matte(1., bits(coverage(s)));
}

@fragment fn owned(s: Surface) -> Matte {
    var mask = 0u;
    for (var i = 0u;i<4u;i++) {
        let code = u32(round(textureLoad(owners, vec2i(s.clip.xy), i32(i)).r * 255.));
        mask |= select(0u, 1u<<i, code == s.owner);
    }
    return Matte(1., mask&bits(coverage(s)));
}
