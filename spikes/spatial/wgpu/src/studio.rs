//! Directional shadows, editable surface inputs and a floating-point studio finish.
use super::*;
use std::ops::Range;
use wgpu::{BindingType, BufferBindingType, ShaderStages, TextureFormat, TextureSampleType};
const LINEAR: TextureFormat = TextureFormat::Rgba16Float;
const SHADOW_SIZE: u32 = 1024;

#[derive(Clone, Copy, Debug)]
pub struct Studio {
    pub environment_rotation_degrees: f32,
    pub environment_intensity: f32,
    pub exposure_ev: f32,
    pub shadow_center: [f32; 3],
    /// Orthographic shadow camera half-width in meters.
    pub shadow_extent: f32,
    /// Receiver bias in meters; slope increases it at grazing angles.
    pub shadow_bias: f32,
    /// PCF radius in shadow texels.
    pub shadow_softness: f32,
}
impl Default for Studio {
    fn default() -> Self {
        Self {
            environment_rotation_degrees: 0.,
            environment_intensity: 1.,
            exposure_ev: 0.,
            shadow_center: [0.; 3],
            shadow_extent: 5.,
            shadow_bias: 0.003,
            shadow_softness: 1.,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum SurfaceMode {
    /// Composite the artwork into the material's linear base color; transparent lettering reveals the underlying paint.
    BaseColor,
    /// The artwork supplies emitted color and sample coverage independently of incident light.
    Unlit,
}
#[derive(Clone, Copy)]
pub struct SurfaceBinding<'a> {
    pub slot: &'a str,
    /// Single-sampled view with linear premultiplied RGB when sampled. UVs follow the imported geometry.
    pub texture: &'a wgpu::TextureView,
    pub mode: SurfaceMode,
}
#[derive(Clone, Copy, Debug)]
pub struct PartPose<'a> {
    pub path: &'a str,
    /// Additional local Euler rotation, in degrees, around the imported pivot.
    pub rotation_degrees: [f32; 3],
}

pub(super) fn texture<'a>(binding: u32, v: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    }
}
pub(super) fn sampler<'a>(binding: u32, v: &'a wgpu::Sampler) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Sampler(v),
    }
}
pub(super) fn attachment<'a>(
    view: &'a wgpu::TextureView,
    resolve_target: Option<&'a wgpu::TextureView>,
) -> Option<wgpu::RenderPassColorAttachment<'a>> {
    Some(wgpu::RenderPassColorAttachment {
        view,
        resolve_target,
        depth_slice: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: if resolve_target.is_some() {
                wgpu::StoreOp::Discard
            } else {
                wgpu::StoreOp::Store
            },
        },
    })
}
pub(super) fn fullscreen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    fragment: &str,
    format: TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: None,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("fullscreen"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            targets: &[Some(format.into())],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
fn make_texture(
    device: &wgpu::Device,
    label: &str,
    size: wgpu::Extent3d,
    format: TextureFormat,
    samples: u32,
    sampled: bool,
) -> wgpu::TextureView {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if sampled {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING;
    }
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
fn layout(
    device: &wgpu::Device,
    label: &str,
    bindings: &[(u32, BindingType, ShaderStages)],
) -> wgpu::BindGroupLayout {
    let entries: Vec<_> = bindings
        .iter()
        .map(|&(binding, ty, visibility)| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}
fn buf(ty: BufferBindingType) -> BindingType {
    BindingType::Buffer {
        ty,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}
fn tex(sample_type: TextureSampleType, multisampled: bool) -> BindingType {
    BindingType::Texture {
        sample_type,
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled,
    }
}
fn group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    entries: &[wgpu::BindGroupEntry<'_>],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries,
    })
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalized(a: [f32; 3]) -> Result<[f32; 3]> {
    let n = dot(a, a).sqrt();
    ensure!(
        n.is_finite() && n > 1e-8,
        "expected a finite nonzero spatial direction"
    );
    Ok(a.map(|v| v / n))
}
fn xyz(a: [f32; 4]) -> [f32; 3] {
    [a[0], a[1], a[2]]
}
impl Pose {
    fn vector(self, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            v[0] * self.columns[1][i] + v[1] * self.columns[2][i] + v[2] * self.columns[3][i]
        })
    }
    fn compose(self, child: Self) -> Self {
        let position = self.vector(xyz(child.columns[0]));
        let mut columns = child.columns.map(|v| four(self.vector(xyz(v)), 0.));
        columns[0] = four(
            std::array::from_fn(|i| position[i] + self.columns[0][i]),
            1.,
        );
        Self { columns }
    }
}
fn part_poses(mesh: &MeshBuffers, poses: &[PartPose<'_>], instance: Pose) -> Result<Vec<Pose>> {
    let mut rotations = vec![None; mesh.studio.parts.len()];
    for edit in poses {
        let i = mesh
            .studio
            .parts
            .iter()
            .position(|p| p.path == edit.path)
            .with_context(|| format!("unknown rigid part {} in this asset", edit.path))?;
        ensure!(
            rotations[i].is_none() && edit.rotation_degrees.iter().all(|x| x.is_finite()),
            "invalid or duplicate rigid part override {}",
            edit.path
        );
        rotations[i] = Some(edit.rotation_degrees);
    }
    let mut results = Vec::with_capacity(rotations.len());
    for (i, part) in mesh.studio.parts.iter().enumerate() {
        let parent = part.parent.map_or(instance, |p| results[p]);
        if let Some(angles) = rotations[i] {
            // Geometry is in asset rest space. Conjugate a local rotation by the
            // authored orthonormal axes and pivot, then inherit the parent's deformation.
            let x = normalized(xyz(part.frame[0]))?;
            let y = normalized(xyz(part.frame[1]))?;
            let z = normalized(xyz(part.frame[2]))?;
            ensure!(
                dot(x, y).abs() < 0.001 && dot(x, z).abs() < 0.001 && dot(y, z).abs() < 0.001,
                "sheared frame cannot receive rigid rotation: {}",
                part.path
            );
            let frame = Pose {
                columns: [four(part.pivot, 1.), four(x, 0.), four(y, 0.), four(z, 0.)],
            };
            let inv = Pose {
                columns: [
                    four(
                        [
                            -dot(x, part.pivot),
                            -dot(y, part.pivot),
                            -dot(z, part.pivot),
                        ],
                        1.,
                    ),
                    [x[0], y[0], z[0], 0.],
                    [x[1], y[1], z[1], 0.],
                    [x[2], y[2], z[2], 0.],
                ],
            };
            results.push(
                parent
                    .compose(frame)
                    .compose(Pose::rigid([0.; 3], angles))
                    .compose(inv),
            );
        } else {
            results.push(parent);
        }
    }
    Ok(results)
}

struct Quad {
    v: wgpu::Buffer,
    i: wgpu::Buffer,
    material: wgpu::Buffer,
}
impl Quad {
    fn new(device: &wgpu::Device) -> Self {
        let mut v = Vec::new();
        for [x, y] in [[0., 0.], [1., 0.], [1., 1.], [0., 1.]] {
            v.extend(float_bytes([x, y, 0., 0., 0., 1.]));
            v.extend(0u32.to_ne_bytes());
            v.extend(float_bytes([x, 1. - y]));
        }
        Self {
            v: buffer(device, "artwork quad", wgpu::BufferUsages::VERTEX, &v),
            i: buffer(
                device,
                "artwork indices",
                wgpu::BufferUsages::INDEX,
                &[0u32, 1, 2, 0, 2, 3]
                    .into_iter()
                    .flat_map(u32::to_ne_bytes)
                    .collect::<Vec<_>>(),
            ),
            material: uniform(device, "artwork material", [[0.; 4]; 19]),
        }
    }
}
struct StudioTargets {
    size: wgpu::Extent3d,
    color: wgpu::TextureView,
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    owners: Option<wgpu::TextureView>,
    coverage: Option<wgpu::TextureView>,
}
pub(super) struct StudioRenderer {
    surface_layout: wgpu::BindGroupLayout,
    view_layout: wgpu::BindGroupLayout,
    lighting_layout: wgpu::BindGroupLayout,
    ownership_layout: wgpu::BindGroupLayout,
    beauty: wgpu::RenderPipeline,
    tagged: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    projected: wgpu::RenderPipeline,
    owned: wgpu::RenderPipeline,
    display: wgpu::RenderPipeline,
    shadow_map: wgpu::TextureView,
    blank: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    map_sampler: wgpu::Sampler,
    art_sampler: wgpu::Sampler,
    quad: Quad,
    targets: Option<StudioTargets>,
}
impl StudioRenderer {
    fn new(device: &wgpu::Device) -> Self {
        let vf = ShaderStages::VERTEX_FRAGMENT;
        let f = ShaderStages::FRAGMENT;
        let uniform = buf(BufferBindingType::Uniform);
        let filtered = tex(TextureSampleType::Float { filterable: true }, false);
        let s = BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let mut attrs = vec![(0, uniform, vf), (1, uniform, f)];
        for i in 2..8 {
            attrs.push((i, filtered, f));
        }
        attrs.extend([(8, s, f), (9, s, f)]);
        let surface_layout = layout(device, "spatial surface inputs", &attrs);
        let view_layout = layout(device, "spatial viewpoint", &[(0, uniform, vf)]);
        let lighting_layout = layout(
            device,
            "studio lighting",
            &[
                (0, uniform, vf),
                (1, buf(BufferBindingType::Storage { read_only: true }), f),
                (2, tex(TextureSampleType::Depth, false), f),
                (
                    3,
                    BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    f,
                ),
            ],
        );
        let ownership_layout = layout(
            device,
            "spatial ownership",
            &[
                (0, uniform, vf),
                (
                    4,
                    tex(TextureSampleType::Float { filterable: false }, true),
                    f,
                ),
            ],
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio surfaces and shadow camera"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/studio.wgsl").into()),
        });
        let make = |global: &wgpu::BindGroupLayout,
                    vs,
                    fs,
                    formats: &[TextureFormat],
                    depth: Option<wgpu::CompareFunction>,
                    samples| {
            let attrs = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Uint32,3=>Float32x2];
            let targets: Vec<_> = formats.iter().map(|&f| Some(f.into())).collect();
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(fs),
                bind_group_layouts: &[Some(global), Some(&surface_layout)],
                ..Default::default()
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 36,
                        attributes: &attrs,
                        step_mode: wgpu::VertexStepMode::Vertex,
                    })],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &targets,
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: depth.map(|c| wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(c),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let beauty = make(
            &lighting_layout,
            "vertex",
            "beauty",
            &[LINEAR],
            Some(wgpu::CompareFunction::Greater),
            SAMPLES,
        );
        let tagged = make(
            &lighting_layout,
            "vertex",
            "tagged",
            &[LINEAR, MATTE_FORMAT],
            Some(wgpu::CompareFunction::Greater),
            SAMPLES,
        );
        let shadow = make(
            &view_layout,
            "shadow_vertex",
            "depth_only",
            &[],
            Some(wgpu::CompareFunction::Less),
            1,
        );
        let projected = make(
            &view_layout,
            "vertex",
            "projected",
            &[MATTE_FORMAT],
            None,
            SAMPLES,
        );
        let owned = make(
            &ownership_layout,
            "vertex",
            "owned",
            &[MATTE_FORMAT],
            None,
            SAMPLES,
        );
        let film = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("linear studio output to SDR"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/film.wgsl").into()),
        });
        let display = fullscreen_pipeline(device, &film, "display", COLOR_FORMAT);
        let size = |w| wgpu::Extent3d {
            width: w,
            height: w,
            depth_or_array_layers: 1,
        };
        let linear = wgpu::FilterMode::Linear;
        Self {
            surface_layout,
            view_layout,
            lighting_layout,
            ownership_layout,
            beauty,
            tagged,
            shadow,
            projected,
            owned,
            display,
            quad: Quad::new(device),
            targets: None,
            shadow_map: make_texture(
                device,
                "key light shadow map",
                size(SHADOW_SIZE),
                DEPTH,
                1,
                true,
            ),
            blank: make_texture(
                device,
                "unused surface input",
                size(1),
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                true,
            ),
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                compare: Some(wgpu::CompareFunction::LessEqual),
                mag_filter: linear,
                min_filter: linear,
                ..Default::default()
            }),
            map_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                min_filter: linear,
                mag_filter: linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            art_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: linear,
                min_filter: linear,
                ..Default::default()
            }),
        }
    }
    fn prepare(&mut self, device: &wgpu::Device, size: wgpu::Extent3d, matte: bool, owners: bool) {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(StudioTargets {
                size,
                color: make_texture(device, "scene-linear samples", size, LINEAR, SAMPLES, false),
                hdr: make_texture(device, "resolved HDR", size, LINEAR, 1, true),
                depth: make_texture(device, "studio sample depth", size, DEPTH, SAMPLES, false),
                owners: None,
                coverage: None,
            });
        }
        let t = self.targets.as_mut().unwrap();
        if matte && t.coverage.is_none() {
            t.coverage = Some(make_texture(
                device,
                "studio matte samples",
                size,
                MATTE_FORMAT,
                SAMPLES,
                false,
            ));
        }
        if owners && t.owners.is_none() {
            t.owners = Some(make_texture(
                device,
                "studio owner samples",
                size,
                MATTE_FORMAT,
                SAMPLES,
                true,
            ));
        }
    }

    fn surface_group(
        &self,
        device: &wgpu::Device,
        pose: Pose,
        mode: f32,
        inputs: &wgpu::Buffer,
        maps: [Option<&wgpu::TextureView>; 5],
        art: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        let [o, x, y, z] = pose.columns;
        let u = uniform(
            device,
            "rigid surface placement",
            [o, x, y, z, [mode, 0., 0., 0.]],
        );
        let mut entries = vec![binding(0, &u), binding(1, inputs)];
        for (i, map) in maps.iter().enumerate() {
            entries.push(texture(i as u32 + 2, map.unwrap_or(&self.blank)));
        }
        entries.extend([
            texture(7, art.unwrap_or(&self.blank)),
            sampler(8, &self.map_sampler),
            sampler(9, &self.art_sampler),
        ]);
        group(device, &self.surface_layout, &entries)
    }
    fn draws<'a>(
        &'a self,
        device: &wgpu::Device,
        objects: &'a [Object<'a>],
    ) -> Result<Vec<StudioDraw<'a>>> {
        let mut draws = Vec::new();
        for object in objects {
            let (mesh, parts, surfaces) = match object.geometry {
                Geometry::Plane(art) => {
                    draws.push(StudioDraw {
                        id: object.id,
                        v: &self.quad.v,
                        i: &self.quad.i,
                        range: 0..6,
                        group: self.surface_group(
                            device,
                            object.pose,
                            3.,
                            &self.quad.material,
                            [None; 5],
                            Some(art),
                        ),
                    });
                    continue;
                }
                Geometry::Mesh(mesh) => (mesh, &[][..], &[][..]),
                Geometry::Model {
                    mesh,
                    parts,
                    surfaces,
                } => (mesh, parts, surfaces),
            };
            let mut overrides = vec![None; mesh.studio.materials.len()];
            for surface in surfaces {
                let i = mesh
                    .studio
                    .materials
                    .iter()
                    .position(|m| m.slot == surface.slot)
                    .with_context(|| {
                        format!(
                            "material slot {} does not exist; select an imported slot",
                            surface.slot
                        )
                    })?;
                ensure!(
                    overrides[i].is_none(),
                    "duplicate material binding {}",
                    surface.slot
                );
                overrides[i] = Some(surface);
            }
            let poses = part_poses(mesh, parts, object.pose)?;
            for batch in &mesh.studio.batches {
                let m = &mesh.studio.materials[batch.material];
                let pose = batch.part.map_or(object.pose, |i| poses[i]);
                let art = overrides[batch.material];
                let mode = match art.map(|s| s.mode) {
                    None => 0.,
                    Some(SurfaceMode::BaseColor) => 1.,
                    Some(SurfaceMode::Unlit) => 2.,
                };
                let maps = m.maps.map(|k| k.map(|k| &mesh.studio.textures[&k]));
                draws.push(StudioDraw {
                    id: object.id,
                    v: &mesh.vertices,
                    i: &mesh.studio.indices,
                    range: batch.indices.clone(),
                    group: self.surface_group(
                        device,
                        pose,
                        mode,
                        &m.inputs,
                        maps,
                        art.map(|s| s.texture),
                    ),
                });
            }
        }
        Ok(draws)
    }
}
struct StudioDraw<'a> {
    id: Option<&'a str>,
    v: &'a wgpu::Buffer,
    i: &'a wgpu::Buffer,
    range: Range<u32>,
    group: wgpu::BindGroup,
}
impl StudioDraw<'_> {
    fn encode(&self, pass: &mut wgpu::RenderPass<'_>, owner: u32) {
        pass.set_vertex_buffer(0, self.v.slice(..));
        pass.set_index_buffer(self.i.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(1, &self.group, &[]);
        pass.draw_indexed(self.range.clone(), 0, owner..owner + 1);
    }
}

impl Renderer {
    /// Record the studio shadow, scene-linear beauty, requested mattes and SDR display transform.
    #[allow(clippy::too_many_arguments)]
    pub fn record_studio(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        camera: View,
        objects: &[Object<'_>],
        lights: &[Light],
        matte_output: Option<MatteOutput<'_>>,
        studio: &Studio,
    ) -> Result<()> {
        validate_view(target, camera, matte_output.as_ref())?;
        ensure!(
            studio.shadow_extent.is_finite()
                && studio.shadow_extent > 0.
                && studio.shadow_bias.is_finite()
                && studio.shadow_bias >= 0.
                && studio.shadow_softness.is_finite()
                && studio.shadow_softness >= 0.
                && studio.environment_intensity.is_finite()
                && studio.environment_intensity >= 0.
                && studio.environment_rotation_degrees.is_finite()
                && studio.shadow_center.iter().all(|x| x.is_finite())
                && studio.exposure_ev.exp2().is_finite(),
            "invalid studio light, shadow volume or exposure"
        );
        let mattes = matte_output.as_ref().map_or(&[][..], |m| m.requests);
        let need_owners = mattes.iter().any(|m| m.owner.is_some());
        matte_scratch_bytes([target.width(), target.height()], mattes.len(), need_owners)?;
        let d = &self.device;
        let pipeline = self.studio.get_or_insert_with(|| StudioRenderer::new(d));
        pipeline.prepare(d, target.size(), !mattes.is_empty(), need_owners);
        let t = pipeline.targets.as_ref().unwrap();
        let draws = pipeline.draws(d, objects)?;
        let sun = normalized(lights.first().map_or([0., -1., 0.], |l| l.direction))?;
        let up = if sun[1].abs() > 0.95 {
            [0., 0., 1.]
        } else {
            [0., 1., 0.]
        };
        let sx = normalized(cross(sun, up))?;
        let sy = cross(sx, sun);
        let [x, y, z] = axes(camera.rotation_degrees);
        let cot = 1. / (0.5 * camera.fov_y_degrees.to_radians()).tan();
        let (s, c) = studio.environment_rotation_degrees.to_radians().sin_cos();
        let view = uniform(
            d,
            "studio and key camera",
            [
                four(camera.position, 1.),
                x,
                y,
                z,
                [
                    cot * target.height() as f32 / target.width() as f32,
                    cot,
                    0.01,
                    0.,
                ],
                four(studio.shadow_center, studio.shadow_extent),
                four(sx, studio.shadow_bias),
                four(sy, studio.shadow_softness),
                four(sun, f32::from(!lights.is_empty())),
                [c, s, studio.environment_intensity, 0.],
            ],
        );
        let mut values = Vec::new();
        for light in lights {
            values.extend(four(normalized(light.direction)?.map(|v| -v), 0.));
            values.extend(four(light.radiance, 0.));
        }
        if values.is_empty() {
            values.resize(8, 0.);
        }
        let light_buffer = buffer(
            d,
            "studio direct lights",
            wgpu::BufferUsages::STORAGE,
            &float_bytes(values),
        );
        let viewpoint = group(d, &pipeline.view_layout, &[binding(0, &view)]);
        if !lights.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("key light depth"),
                depth_stencil_attachment: Some(depth_target(&pipeline.shadow_map, 1., true)),
                ..Default::default()
            });
            pass.set_pipeline(&pipeline.shadow);
            pass.set_bind_group(0, &viewpoint, &[]);
            for dr in &draws {
                dr.encode(&mut pass, 0);
            }
        }
        let lighting = group(
            d,
            &pipeline.lighting_layout,
            &[
                binding(0, &view),
                binding(1, &light_buffer),
                texture(2, &pipeline.shadow_map),
                sampler(3, &pipeline.shadow_sampler),
            ],
        );
        let owner_code = |id: Option<&str>| {
            id.and_then(|id| mattes.iter().position(|m| m.owner == Some(id)))
                .map_or(0, |i| i as u32 + 1)
        };
        {
            let mut targets = vec![attachment(&t.color, Some(&t.hdr))];
            if need_owners {
                targets.push(attachment(t.owners.as_ref().unwrap(), None));
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("linear studio scene"),
                color_attachments: &targets,
                depth_stencil_attachment: Some(depth_target(&t.depth, 0., false)),
                ..Default::default()
            });
            pass.set_pipeline(if need_owners {
                &pipeline.tagged
            } else {
                &pipeline.beauty
            });
            pass.set_bind_group(0, &lighting, &[]);
            for dr in &draws {
                dr.encode(&mut pass, owner_code(dr.id));
            }
        }
        if let Some(output) = matte_output {
            let ownership = if need_owners {
                Some(group(
                    d,
                    &pipeline.ownership_layout,
                    &[binding(0, &view), texture(4, t.owners.as_ref().unwrap())],
                ))
            } else {
                None
            };
            for (i, m) in output.requests.iter().enumerate() {
                let layer = output.target.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i as u32,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("studio object matte"),
                    color_attachments: &[attachment(t.coverage.as_ref().unwrap(), Some(&layer))],
                    ..Default::default()
                });
                if m.owner.is_some() {
                    pass.set_pipeline(&pipeline.owned);
                    pass.set_bind_group(0, ownership.as_ref().unwrap(), &[]);
                } else {
                    pass.set_pipeline(&pipeline.projected);
                    pass.set_bind_group(0, &viewpoint, &[]);
                }
                for dr in draws.iter().filter(|dr| dr.id == Some(m.object)) {
                    dr.encode(&mut pass, owner_code(m.owner));
                }
            }
        }
        let exposure = uniform(
            d,
            "studio display exposure",
            [[studio.exposure_ev.exp2(), 0., 0., 0.]],
        );
        let display = group(
            d,
            &pipeline.display.get_bind_group_layout(0),
            &[texture(0, &t.hdr), binding(1, &exposure)],
        );
        let out = target.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("studio display transform"),
            color_attachments: &[attachment(&out, None)],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline.display);
        pass.set_bind_group(0, &display, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
fn depth_target(
    view: &wgpu::TextureView,
    clear: f32,
    store: bool,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Clear(clear),
            store: if store {
                wgpu::StoreOp::Store
            } else {
                wgpu::StoreOp::Discard
            },
        }),
        stencil_ops: None,
    }
}
