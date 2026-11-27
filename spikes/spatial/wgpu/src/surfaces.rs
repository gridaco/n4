//! Immutable GPU resources shared by all placements of an imported asset.
use super::{buffer, four, uniform};
use anyhow::{Result, ensure};
use nothing_spatial::{Image, Mesh, TextureChannel, TextureMap};
use std::{collections::BTreeMap, ops::Range};

pub(super) struct MaterialSurface {
    pub slot: String,
    pub inputs: wgpu::Buffer,
    // Base color, tangent normal, roughness, metallic, ambient occlusion.
    pub maps: [Option<(usize, bool)>; 5],
}
pub(super) struct Part {
    pub path: String,
    pub parent: Option<usize>,
    pub frame: [[f32; 4]; 4],
    pub pivot: [f32; 3],
}
pub(super) struct Batch {
    pub part: Option<usize>,
    pub material: usize,
    pub indices: Range<u32>,
}
pub(super) struct MeshSurfaces {
    pub has_maps: bool,
    pub parts: Vec<Part>,
    pub materials: Vec<MaterialSurface>,
    pub textures: BTreeMap<(usize, bool), wgpu::TextureView>,
    pub batches: Vec<Batch>,
    pub indices: wgpu::Buffer,
}

fn map_parameters(map: Option<&TextureMap>) -> [[f32; 4]; 3] {
    let Some(m) = map else { return [[0.; 4]; 3] };
    let channel = match m.channel {
        TextureChannel::Rgb => 0.,
        TextureChannel::R => 1.,
        TextureChannel::G => 2.,
        TextureChannel::B => 3.,
        TextureChannel::A => 4.,
    };
    [[1., channel, 0., 0.], m.scale, m.bias]
}

impl MeshSurfaces {
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Result<Self> {
        let mut textures = BTreeMap::new();
        let mut materials = Vec::new();
        let mut mipmaps = Mipmaps::default();
        for m in &mesh.materials {
            let maps = [
                &m.maps.diffuse,
                &m.maps.normal,
                &m.maps.roughness,
                &m.maps.metallic,
                &m.maps.occlusion,
            ];
            let mut inputs = [[0.; 4]; 19];
            inputs[0] = four(m.diffuse, m.roughness);
            inputs[1] = four(m.emissive, m.metallic);
            inputs[2] = four(m.specular, u8::from(m.use_specular_workflow) as f32);
            inputs[3] = [m.clearcoat, m.clearcoat_roughness, 0., 0.];
            for (index, map) in maps.iter().enumerate() {
                inputs[4 + 3 * index..7 + 3 * index].copy_from_slice(&map_parameters(map.as_ref()));
                if let Some(map) = map
                    && let std::collections::btree_map::Entry::Vacant(entry) =
                        textures.entry((map.image, map.srgb))
                {
                    let image = mesh.images.get(map.image).ok_or_else(|| {
                        anyhow::anyhow!(
                            "material {} has an invalid texture index {}",
                            m.slot,
                            map.image
                        )
                    })?;
                    entry.insert(mipmaps.upload(device, queue, image, map.srgb)?);
                }
            }
            materials.push(MaterialSurface {
                slot: m.slot.clone(),
                inputs: uniform(device, "studio material and UV readers", inputs),
                maps: maps.map(|m| m.as_ref().map(|m| (m.image, m.srgb))),
            });
        }
        let mut part_at = vec![None; mesh.indices.len() / 3];
        let mut parts = Vec::new();
        for (i, part) in mesh.parts.iter().enumerate() {
            ensure!(
                part.indices.start <= part.indices.end
                    && part.indices.end as usize <= mesh.indices.len()
                    && part.indices.start % 3 == 0
                    && part.indices.end % 3 == 0,
                "invalid index range for {}",
                part.path
            );
            ensure!(
                part.parent.is_none_or(|p| p < i),
                "part parents must precede their children: {}",
                part.path
            );
            for entry in
                &mut part_at[part.indices.start as usize / 3..part.indices.end as usize / 3]
            {
                ensure!(
                    entry.is_none(),
                    "multiple parts own triangles of {}",
                    part.path
                );
                *entry = Some(i);
            }
            parts.push(Part {
                path: part.path.clone(),
                parent: part.parent,
                frame: part.frame,
                pivot: part.pivot,
            });
        }
        // Preserve primitive order within each rigid part and material; most products need
        // a handful of draws even when face subsets alternate throughout the source indices.
        let mut groups = BTreeMap::<(Option<usize>, usize), Vec<u32>>::new();
        for (t, triangle) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            let mat = mesh.vertices[triangle[0] as usize].material;
            ensure!(
                triangle
                    .iter()
                    .all(|&v| mesh.vertices[v as usize].material == mat),
                "a triangle crosses material slots"
            );
            groups
                .entry((part_at[t], mat as usize))
                .or_default()
                .extend(triangle);
        }
        let mut indices = Vec::with_capacity(mesh.indices.len() * 4);
        let mut batches = Vec::new();
        for ((part, material), group) in groups {
            let start = (indices.len() / 4) as u32;
            indices.extend(group.iter().flat_map(|i| i.to_ne_bytes()));
            batches.push(Batch {
                part,
                material,
                indices: start..(indices.len() / 4) as u32,
            });
        }
        Ok(Self {
            has_maps: !textures.is_empty(),
            parts,
            materials,
            batches,
            textures,
            indices: buffer(
                device,
                "studio part and material indices",
                wgpu::BufferUsages::INDEX,
                &indices,
            ),
        })
    }
}

// Build the chain on the GPU. Sampling and rendering through sRGB views averages
// base color in linear light; scalar maps use raw UNORM throughout.
#[derive(Default)]
struct Mipmaps {
    raw: Option<wgpu::RenderPipeline>,
    color: Option<wgpu::RenderPipeline>,
}
impl Mipmaps {
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &Image,
        srgb: bool,
    ) -> Result<wgpu::TextureView> {
        let (w, h) = (image.width, image.height);
        ensure!(
            w > 0
                && h > 0
                && w.max(h) <= device.limits().max_texture_dimension_2d
                && image.rgba.len() as u64 == u64::from(w) * u64::from(h) * 4,
            "invalid image dimensions for {}",
            image.source
        );
        let format = if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let count = w.max(h).ilog2() + 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&image.source),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
            texture.size(),
        );
        if count > 1 {
            let pipeline = if srgb { &mut self.color } else { &mut self.raw };
            let pipeline = pipeline.get_or_insert_with(|| {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("mipmap generation"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mipmap.wgsl").into()),
                });
                super::studio::fullscreen_pipeline(device, &shader, "downsample", format)
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                min_filter: wgpu::FilterMode::Linear,
                mag_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            let view = |level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            };
            let mut previous = view(0);
            for level in 1..count {
                let next = view(level);
                let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("mip source"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        super::studio::texture(0, &previous),
                        super::studio::sampler(1, &sampler),
                    ],
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("linear-light mip"),
                    color_attachments: &[super::studio::attachment(&next, None)],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &inputs, &[]);
                pass.draw(0..3, 0..1);
                drop(pass);
                previous = next;
            }
            queue.submit([encoder.finish()]);
        }
        Ok(texture.create_view(&Default::default()))
    }
}
