//! GPU passes for opaque meshes and coverage-masked artwork in a right-handed, Y-up world in meters.
//! The caller supplies targets and records or submits subsequent work on the same device.
mod studio;
mod surfaces;
use anyhow::{Context, Result, ensure};
use nothing_spatial::Mesh;
pub use studio::{PartPose, Studio, SurfaceBinding, SurfaceMode};
use wgpu::util::DeviceExt;

/// Single-sampled color target. Coverage resolves to premultiplied color on transparent black.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SAMPLES: u32 = 4;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub struct MeshBuffers {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    materials: wgpu::Buffer,
    count: u32,
    studio: surfaces::MeshSurfaces,
}

struct GeometryPipelines {
    mesh: wgpu::RenderPipeline,
    plane: wgpu::RenderPipeline,
}

pub struct Renderer {
    device: wgpu::Device,
    shader: wgpu::ShaderModule,
    beauty: GeometryPipelines,
    studio: Option<studio::StudioRenderer>,
    sampler: wgpu::Sampler,
    surfaces: Option<Surfaces>,
    mattes: Option<MattePipelines>,
}

struct MattePipelines {
    tagged: GeometryPipelines,
    projected: GeometryPipelines,
    owned: GeometryPipelines,
    surfaces: Option<MatteSurfaces>,
}

struct MatteSurfaces {
    owners: Option<wgpu::TextureView>,
    coverage: wgpu::TextureView,
    size: wgpu::Extent3d,
}
struct Surfaces {
    color: wgpu::Texture,
    depth: wgpu::Texture,
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    usage: wgpu::BufferUsages,
    bytes: &[u8],
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        usage,
        contents: bytes,
    })
}

fn float_bytes(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    values.into_iter().flat_map(f32::to_ne_bytes).collect()
}

impl MeshBuffers {
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Result<Self> {
        ensure!(
            !mesh.vertices.is_empty() && !mesh.indices.is_empty() && !mesh.materials.is_empty(),
            "mesh requires vertices, triangle indices and materials"
        );
        ensure!(
            mesh.indices.len().is_multiple_of(3)
                && mesh
                    .indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.vertices.len()),
            "invalid triangle indices"
        );
        ensure!(
            mesh.vertices
                .iter()
                .all(|v| (v.material as usize) < mesh.materials.len()),
            "vertex refers to an absent material"
        );
        let mut vertices = Vec::with_capacity(mesh.vertices.len() * 36);
        for v in &mesh.vertices {
            for f in v.position.into_iter().chain(v.normal) {
                vertices.extend_from_slice(&f.to_ne_bytes());
            }
            vertices.extend_from_slice(&v.material.to_ne_bytes());
            vertices.extend(float_bytes(v.uv));
        }
        let indices: Vec<_> = mesh.indices.iter().flat_map(|i| i.to_ne_bytes()).collect();
        let mut materials = Vec::with_capacity(mesh.materials.len() * 64);
        for m in &mesh.materials {
            let vectors = [
                four(m.diffuse, m.roughness),
                four(m.emissive, m.metallic),
                four(m.specular, u8::from(m.use_specular_workflow) as f32),
                [m.clearcoat, m.clearcoat_roughness, 0., 0.],
            ];
            materials.extend(float_bytes(vectors.into_iter().flatten()));
        }
        Ok(Self {
            vertices: buffer(
                device,
                "spatial mesh vertices",
                wgpu::BufferUsages::VERTEX,
                &vertices,
            ),
            indices: buffer(
                device,
                "spatial mesh indices",
                wgpu::BufferUsages::INDEX,
                &indices,
            ),
            materials: buffer(
                device,
                "spatial surface materials",
                wgpu::BufferUsages::STORAGE,
                &materials,
            ),
            count: mesh.indices.len().try_into()?,
            studio: surfaces::MeshSurfaces::upload(device, queue, mesh)?,
        })
    }
}

fn four(x: [f32; 3], w: f32) -> [f32; 4] {
    [x[0], x[1], x[2], w]
}

/// Columns of Rz * Ry * Rx. The inverse rotation of an orthonormal camera uses their dot products.
fn axes([x, y, z]: [f32; 3]) -> [[f32; 4]; 3] {
    let (sx, cx) = x.to_radians().sin_cos();
    let (sy, cy) = y.to_radians().sin_cos();
    let (sz, cz) = z.to_radians().sin_cos();
    [
        [cz * cy, sz * cy, -sy, 0.],
        [cz * sy * sx - sz * cx, sz * sy * sx + cz * cx, cy * sx, 0.],
        [cz * sy * cx + sz * sx, sz * sy * cx - cz * sx, cy * cx, 0.],
    ]
}

fn uniform<const N: usize>(
    device: &wgpu::Device,
    label: &str,
    data: [[f32; 4]; N],
) -> wgpu::Buffer {
    buffer(
        device,
        label,
        wgpu::BufferUsages::UNIFORM,
        &float_bytes(data.into_iter().flatten()),
    )
}

/// A sampled perspective camera. With zero rotation it looks down the negative Z axis.
#[derive(Clone, Copy)]
pub struct View {
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub fov_y_degrees: f32,
}

#[derive(Clone, Copy)]
pub struct Light {
    /// Direction in which light travels.
    pub direction: [f32; 3],
    /// Linear-light RGB including intensity.
    pub radiance: [f32; 3],
}

#[derive(Clone, Copy)]
pub struct Pose {
    columns: [[f32; 4]; 4],
}

impl Pose {
    pub fn rigid(position: [f32; 3], rotation_degrees: [f32; 3]) -> Self {
        let [x, y, z] = axes(rotation_degrees);
        Self {
            columns: [four(position, 1.), x, y, z],
        }
    }

    /// Origin is the top-left corner. Right and down span the complete texture.
    pub fn plane(origin: [f32; 3], right: [f32; 3], down: [f32; 3]) -> Self {
        Self {
            columns: [
                four(origin, 1.),
                four(right, 0.),
                four(down, 0.),
                [0., 0., 1., 0.],
            ],
        }
    }
}

pub struct Object<'a> {
    /// Unique within the sampled scene when an object can be addressed by a matte.
    pub id: Option<&'a str>,
    pub pose: Pose,
    pub geometry: Geometry<'a>,
}

pub struct Matte<'a> {
    /// Subject to project, including its coverage mask. An absent subject produces an empty matte.
    pub object: &'a str,
    /// Restrict coverage to samples where this name owns the nearest surface.
    pub owner: Option<&'a str>,
}

/// Each request writes one layer, in order, of the caller's single-sampled matte target.
pub struct MatteOutput<'a> {
    pub target: &'a wgpu::Texture,
    pub requests: &'a [Matte<'a>],
}

impl GeometryPipelines {
    fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        fragments: [&str; 2],
        formats: &[wgpu::TextureFormat],
        depth: bool,
    ) -> Self {
        let targets: Vec<_> = formats
            .iter()
            .map(|&format| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();
        let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Uint32];
        let vertices = [Some(wgpu::VertexBufferLayout {
            array_stride: 36,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        })];
        let pipeline = |vs, fs, buffers: &[Option<wgpu::VertexBufferLayout>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: None,
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: Default::default(),
                depth_stencil: depth.then_some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Greater),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLES,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            mesh: pipeline("mesh_vertex", fragments[0], &vertices),
            plane: pipeline("plane_vertex", fragments[1], &[]),
        }
    }
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native spatial scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/spatial.wgsl").into()),
        });
        let beauty = GeometryPipelines::new(
            device,
            &shader,
            ["mesh_fragment", "plane_fragment"],
            &[COLOR_FORMAT],
            true,
        );
        Self {
            device: device.clone(),
            shader,
            beauty,
            studio: None,
            mattes: None,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            surfaces: None,
        }
    }

    fn surfaces(&mut self, size: wgpu::Extent3d) -> &Surfaces {
        if self
            .surfaces
            .as_ref()
            .is_none_or(|s| s.color.size() != size)
        {
            let msaa = |label, format| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: SAMPLES,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
            };
            self.surfaces = Some(Surfaces {
                color: msaa("spatial sample color", COLOR_FORMAT),
                depth: msaa("spatial sample depth", DEPTH),
            });
        }
        self.surfaces.as_ref().unwrap()
    }
}

pub const MATTE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

pub fn matte_scratch_bytes(size: [u32; 2], count: usize, needs_owners: bool) -> Result<u64> {
    if count == 0 {
        return Ok(0);
    }
    ensure!(
        count <= usize::from(u8::MAX),
        "matte owner code exceeds the R8 attachment range"
    );
    let bytes_per_pixel = u64::from(SAMPLES) * (1 + u64::from(needs_owners)) + count as u64;
    let pixels = u64::from(size[0]) * u64::from(size[1]);
    let bytes = pixels
        .checked_mul(bytes_per_pixel)
        .context("spatial matte size overflow")?;
    ensure!(
        bytes <= 256 * 1024 * 1024,
        "spatial matte surfaces exceed 256 MiB"
    );
    Ok(bytes)
}

impl MattePipelines {
    fn new(device: &wgpu::Device, shader: &wgpu::ShaderModule) -> Self {
        Self {
            tagged: GeometryPipelines::new(
                device,
                shader,
                ["tagged_mesh", "tagged_plane"],
                &[COLOR_FORMAT, MATTE_FORMAT],
                true,
            ),
            projected: GeometryPipelines::new(
                device,
                shader,
                ["projected_mesh", "projected_plane"],
                &[MATTE_FORMAT],
                false,
            ),
            owned: GeometryPipelines::new(
                device,
                shader,
                ["owned_mesh", "owned_plane"],
                &[MATTE_FORMAT],
                false,
            ),
            surfaces: None,
        }
    }

    fn prepare(&mut self, device: &wgpu::Device, size: wgpu::Extent3d, owners: bool) {
        if self
            .surfaces
            .as_ref()
            .is_some_and(|s| s.size == size && s.owners.is_some() == owners)
        {
            return;
        }
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = attachment | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |label, size, sample_count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: MATTE_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        self.surfaces = Some(MatteSurfaces {
            owners: owners.then(|| {
                texture("spatial sample owners", size, SAMPLES, sampled)
                    .create_view(&Default::default())
            }),
            coverage: texture("spatial sample coverage", size, SAMPLES, attachment)
                .create_view(&Default::default()),
            size,
        });
    }
}

#[derive(Clone, Copy)]
pub enum Geometry<'a> {
    Mesh(&'a MeshBuffers),
    /// Instance-local overrides address imported prims and material slots.
    Model {
        mesh: &'a MeshBuffers,
        parts: &'a [PartPose<'a>],
        surfaces: &'a [SurfaceBinding<'a>],
    },
    /// Filterable RGBA8Unorm view containing premultiplied sRGB artwork (one sample).
    Plane(&'a wgpu::TextureView),
}

struct Draw<'a> {
    id: Option<&'a str>,
    pose: wgpu::Buffer,
    geometry: Geometry<'a>,
}

struct SceneBindings<'a> {
    device: &'a wgpu::Device,
    view: &'a wgpu::Buffer,
    sampler: &'a wgpu::Sampler,
}

enum DrawInputs<'a> {
    Beauty(&'a wgpu::Buffer),
    Projection,
    Owner(&'a wgpu::TextureView),
}

impl Draw<'_> {
    fn encode(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        scene: &SceneBindings<'_>,
        pipelines: &GeometryPipelines,
        inputs: DrawInputs<'_>,
        owner: u32,
    ) {
        let mut entries = vec![binding(0, scene.view), binding(1, &self.pose)];
        let pipeline = match self.geometry {
            Geometry::Mesh(mesh) | Geometry::Model { mesh, .. } => {
                if let DrawInputs::Beauty(lights) = inputs {
                    entries.extend([binding(2, &mesh.materials), binding(3, lights)]);
                }
                &pipelines.mesh
            }
            Geometry::Plane(texture) => {
                entries.extend([
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(texture),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(scene.sampler),
                    },
                ]);
                &pipelines.plane
            }
        };
        if let DrawInputs::Owner(owners) = inputs {
            entries.push(wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(owners),
            });
        }
        let group = scene.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spatial draw inputs"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        let instance = owner..owner + 1;
        match self.geometry {
            Geometry::Mesh(mesh) | Geometry::Model { mesh, .. } => {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, instance);
            }
            Geometry::Plane(_) => pass.draw(0..6, instance),
        }
    }
}

fn binding(index: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding: index,
        resource: buffer.as_entire_binding(),
    }
}

impl Renderer {
    /// Clears and writes the host's 2D, single-sampled color target. Requested mattes use
    /// a matching R8Unorm array with one layer per request. All targets require RENDER_ATTACHMENT.
    /// Each invocation records independent camera and object uniforms; submission belongs to the host.
    pub fn record(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        camera: View,
        objects: &[Object<'_>],
        lights: &[Light],
        matte_output: Option<MatteOutput<'_>>,
    ) -> Result<()> {
        if objects.iter().any(|o| match o.geometry {
            Geometry::Model { .. } => true,
            Geometry::Mesh(mesh) => mesh.studio.has_maps,
            Geometry::Plane(_) => false,
        }) {
            return self.record_studio(
                encoder,
                target,
                camera,
                objects,
                lights,
                matte_output,
                &Studio::default(),
            );
        }
        validate_view(target, camera, matte_output.as_ref())?;
        let mattes = matte_output.as_ref().map_or(&[][..], |m| m.requests);
        let matte_target = matte_output.as_ref().map(|m| m.target);
        let need_owners = mattes.iter().any(|m| m.owner.is_some());
        matte_scratch_bytes([target.width(), target.height()], mattes.len(), need_owners)?;
        let surfaces = self.surfaces(target.size());
        let color = surfaces.color.create_view(&Default::default());
        let depth = surfaces.depth.create_view(&Default::default());
        let output = target.create_view(&Default::default());
        let device = &self.device;
        let draws: Vec<_> = objects
            .iter()
            .map(|object| Draw {
                id: object.id,
                pose: uniform(device, "spatial placement", object.pose.columns),
                geometry: object.geometry,
            })
            .collect();
        let [x, y, z] = axes(camera.rotation_degrees);
        let cot = 1. / (0.5 * camera.fov_y_degrees.to_radians()).tan();
        let view = uniform(
            device,
            "sampled scene view",
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
            ],
        );
        let mut light_data = Vec::with_capacity(lights.len() * 8);
        for light in lights {
            let [x, y, z] = light.direction;
            let n = x.hypot(y).hypot(z);
            ensure!(
                n.is_finite() && n > 0.,
                "distant light requires a nonzero finite direction"
            );
            light_data.extend(four(light.direction.map(|v| -v / n), 0.));
            light_data.extend(four(light.radiance, 0.));
        }
        if light_data.is_empty() {
            light_data.resize(8, 0.);
        }
        let light_buf = buffer(
            device,
            "scene lighting",
            wgpu::BufferUsages::STORAGE,
            &float_bytes(light_data),
        );
        let matte_state = if mattes.is_empty() {
            None
        } else {
            let m = self
                .mattes
                .get_or_insert_with(|| MattePipelines::new(device, &self.shader));
            m.prepare(device, target.size(), need_owners);
            Some(&*m)
        };
        let matte_surfaces = matte_state.and_then(|m| m.surfaces.as_ref());
        let owners = matte_surfaces.and_then(|s| s.owners.as_ref());
        let beauty = matte_state
            .filter(|_| need_owners)
            .map_or(&self.beauty, |m| &m.tagged);
        // Only owners addressed by this filter need distinct codes. Zero also
        // represents the nearest object when its name was not requested.
        let owner_code = |id: Option<&str>| {
            id.and_then(|id| mattes.iter().position(|m| m.owner == Some(id)))
                .map_or(0, |i| i as u32 + 1)
        };
        let mut attachments = vec![Some(wgpu::RenderPassColorAttachment {
            view: &color,
            resolve_target: Some(&output),
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Discard,
            },
        })];
        if let Some(owners) = owners {
            attachments.push(Some(wgpu::RenderPassColorAttachment {
                view: owners,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            }));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render shared spatial scene"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        let bindings = SceneBindings {
            device,
            view: &view,
            sampler: &self.sampler,
        };
        for draw in &draws {
            draw.encode(
                &mut pass,
                &bindings,
                beauty,
                DrawInputs::Beauty(&light_buf),
                owner_code(draw.id),
            );
        }
        drop(pass);

        if let (Some(state), Some(surfaces), Some(layers)) =
            (matte_state, matte_surfaces, matte_target)
        {
            for (index, matte) in mattes.iter().enumerate() {
                let layer = layers.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: index as u32,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("project object into matte layer"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &surfaces.coverage,
                        resolve_target: Some(&layer),
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Discard,
                        },
                    })],
                    ..Default::default()
                });
                // The clear yields an empty matte when an authored plane is inactive.
                if let Some(draw) = draws.iter().find(|d| d.id == Some(matte.object)) {
                    let owner = matte.owner;
                    let (pipeline, inputs) = match owner {
                        Some(_) => (
                            &state.owned,
                            DrawInputs::Owner(owners.context("missing scene ownership surface")?),
                        ),
                        None => (&state.projected, DrawInputs::Projection),
                    };
                    draw.encode(&mut pass, &bindings, pipeline, inputs, owner_code(owner));
                }
            }
        }
        Ok(())
    }
}

fn validate_view(
    target: &wgpu::Texture,
    camera: View,
    matte_output: Option<&MatteOutput<'_>>,
) -> Result<()> {
    ensure!(
        target.format() == COLOR_FORMAT
            && target.sample_count() == 1
            && target.dimension() == wgpu::TextureDimension::D2
            && target.depth_or_array_layers() == 1
            && target.mip_level_count() == 1
            && target
                .usage()
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
        "expected a single-layer, single-level Rgba8UnormSrgb render target"
    );
    if let Some(matte) = matte_output {
        let mask = matte.target;
        ensure!(
            mask.format() == MATTE_FORMAT
                && mask.sample_count() == 1
                && mask.dimension() == wgpu::TextureDimension::D2
                && mask.mip_level_count() == 1
                && mask.width() == target.width()
                && mask.height() == target.height()
                && mask.depth_or_array_layers() as usize == matte.requests.len()
                && mask
                    .usage()
                    .contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
            "expected an R8Unorm render target with a matching extent and one layer per matte"
        );
    }
    ensure!(
        camera.fov_y_degrees > 0. && camera.fov_y_degrees < 180.,
        "perspective fov must be between zero and 180 degrees"
    );
    Ok(())
}
