use super::{ScenePainter, TARGET_FORMAT};
use anyhow::{Result, ensure};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy)]
pub(super) struct Region {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Region {
    pub(super) fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
    pub(super) fn extent(self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }
}

pub(super) fn linear_surface(
    device: &wgpu::Device,
    label: &'static str,
    size: wgpu::Extent3d,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

pub(super) const MAX_SCRATCH_BYTES: u64 = 256 * 1024 * 1024;

/// Both sampled operations are submitted sequentially. Share a bounded GPU
/// workspace across adjustments, layers and exposure samples on this compositor.
pub(super) struct Scratch {
    pub(super) original: wgpu::Texture,
    pub(super) horizontal: wgpu::Texture,
    pub(super) vertical: wgpu::Texture,
    pub(super) bytes: u64,
}

pub(super) fn scratch_bytes([source, rows, output]: [Region; 3]) -> Result<u64> {
    let bytes = source.pixels() * 4 + (rows.pixels() + output.pixels()) * 8;
    ensure!(
        bytes <= MAX_SCRATCH_BYTES,
        "sampled filter needs {bytes} private texture bytes, exceeding the 256 MiB spike limit"
    );
    Ok(bytes)
}

impl Scratch {
    pub(super) fn reserve<'a>(
        slot: &'a mut Option<Self>,
        device: &wgpu::Device,
        regions: [Region; 3],
    ) -> Result<&'a Self> {
        let fits =
            |tex: &wgpu::Texture, r: Region| tex.width() >= r.width && tex.height() >= r.height;
        let [source, rows, output] = regions;
        if slot.as_ref().is_none_or(|s| {
            !fits(&s.original, source) || !fits(&s.horizontal, rows) || !fits(&s.vertical, output)
        }) {
            let bytes = scratch_bytes(regions)?;
            let make = |label, region: Region, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: region.extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            };
            let render = wgpu::TextureUsages::RENDER_ATTACHMENT;
            *slot = Some(Self {
                original: make(
                    "sampled filter input",
                    source,
                    TARGET_FORMAT,
                    render | wgpu::TextureUsages::COPY_DST,
                ),
                horizontal: make(
                    "horizontal filter scratch",
                    rows,
                    wgpu::TextureFormat::Rgba16Float,
                    render,
                ),
                vertical: make(
                    "vertical filter scratch",
                    output,
                    wgpu::TextureFormat::Rgba16Float,
                    render,
                ),
                bytes,
            });
        }
        Ok(slot.as_ref().expect("scratch reserved"))
    }
}
/// The sampled-filter uniform packs nonnegative weights in vec4s so its
/// layout is portable across uniform-buffer backends (support <= 96).
pub(super) fn gaussian_weights(sigma: f32, radius: u32) -> [f32; 100] {
    let mut weights = [0.0; 100];
    weights[0] = 1.0;
    let mut sum = 1.0;
    for i in 1..=radius {
        let t = i as f32 / sigma;
        let w = (-0.5 * t * t).exp();
        weights[i as usize] = w;
        sum += 2.0 * w;
    }
    for w in &mut weights[..=radius as usize] {
        *w /= sum;
    }
    weights
}

pub(super) fn sampling_uniform(
    input: Region,
    output: Region,
    direction: [i32; 2],
    radius: u32,
    weights: &[f32; 100],
    transparent_edges: bool,
) -> Vec<u8> {
    let header = [
        input.x as i32,
        input.y as i32,
        input.width as i32,
        input.height as i32,
        output.x as i32,
        output.y as i32,
        direction[0],
        direction[1],
        radius as i32,
        i32::from(transparent_edges),
        0,
        0,
    ];
    header
        .into_iter()
        .flat_map(i32::to_le_bytes)
        .chain(weights.iter().copied().flat_map(f32::to_le_bytes))
        .collect()
}

pub(super) struct Pass<'a> {
    pub(super) label: &'static str,
    pub(super) inputs: &'a [&'a wgpu::Texture],
    pub(super) output: &'a wgpu::Texture,
    pub(super) viewport: Region,
    pub(super) pipeline: &'a wgpu::RenderPipeline,
    pub(super) uniform: &'a [u8],
    pub(super) preserve: bool,
}

impl ScenePainter<'_> {
    pub(super) fn filter_pass(&mut self, draw: Pass<'_>) {
        let device = &self.gpu.device;
        let views: Vec<_> = draw
            .inputs
            .iter()
            .map(|tex| tex.create_view(&Default::default()))
            .collect();
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(draw.label),
            contents: draw.uniform,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut entries: Vec<_> = views
            .iter()
            .enumerate()
            .map(|(i, view)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect();
        entries.push(wgpu::BindGroupEntry {
            binding: views.len() as u32,
            resource: params.as_entire_binding(),
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(draw.label),
            layout: &draw.pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let dest = draw.output.create_view(&Default::default());
        let mut pass = self
            .commands
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(draw.label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dest,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if draw.preserve {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        let r = draw.viewport;
        pass.set_scissor_rect(r.x, r.y, r.width, r.height);
        pass.set_pipeline(draw.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
