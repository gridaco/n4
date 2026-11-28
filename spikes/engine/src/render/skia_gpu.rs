//! Where a local vector drawing lands: transparent CPU pixels, or a compositor texture that Skia
//! draws on the engine's own wgpu device. Both receive the same canvas calls.
use crate::graphics::Raster;
use anyhow::Result;
use skia_safe::Canvas;

/// Size and origin have been validated by the caller. The canvas is in local units.
pub(crate) trait Painter {
    type Picture;
    fn paint(
        &mut self,
        size: [u32; 2],
        origin: [i32; 2],
        draw: impl FnOnce(&Canvas) -> Result<()>,
    ) -> Result<Self::Picture>;
}

/// Premultiplied RGBA8 in memory. Every consumer that reads pixels uses this.
pub(crate) struct Cpu;

impl Painter for Cpu {
    type Picture = Raster;
    fn paint(
        &mut self,
        size: [u32; 2],
        origin: [i32; 2],
        draw: impl FnOnce(&Canvas) -> Result<()>,
    ) -> Result<Raster> {
        super::path::rasterize(size, origin, draw)
    }
}

/// A texture the compositor samples, and the local position of its first texel.
pub(crate) type Texture = (wgpu::Texture, [i32; 2]);

#[cfg(feature = "skia-gpu")]
mod graphite {
    use super::{Painter, Texture};
    use anyhow::{Context as _, Result, ensure};
    use skia_safe::{Canvas, Color, ColorType, gpu::graphite};

    /// Skia's Graphite renderer on the compositor's device and queue. Its submissions precede
    /// the compositor's own, so a painted texture is complete before a pass samples it.
    pub(crate) struct SkiaGpu {
        device: wgpu::Device,
        context: graphite::Context,
        recorder: graphite::Recorder,
    }

    impl SkiaGpu {
        /// `N4_SKIA_GPU=0` keeps a build that has this painter on the CPU reference.
        pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Option<Self>> {
            if std::env::var_os("N4_SKIA_GPU").is_some_and(|v| v == "0") {
                return Ok(None);
            }
            let mut context = graphite::wgpu::context_factory::make_wgpu(device, queue, None)
                .context("Skia cannot draw on this wgpu device")?;
            let recorder = context
                .make_recorder(None)
                .context("create Skia recorder")?;
            Ok(Some(Self {
                device: device.clone(),
                context,
                recorder,
            }))
        }
    }

    impl Painter for SkiaGpu {
        type Picture = Texture;
        fn paint(
            &mut self,
            size: [u32; 2],
            origin: [i32; 2],
            draw: impl FnOnce(&Canvas) -> Result<()>,
        ) -> Result<Texture> {
            let limit = self.device.limits().max_texture_dimension_2d;
            ensure!(
                size.iter().all(|&n| n > 0 && n <= limit),
                "drawing {} × {} exceeds the supported range 1..={limit}",
                size[0],
                size[1]
            );
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("compositor input drawn by Skia"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mut surface = graphite::wgpu::surfaces::wrap_texture(
                &mut self.recorder,
                &texture,
                ColorType::RGBA8888,
                None,
                None,
            )
            .context("wrap compositor texture for Skia")?;
            let canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            canvas.translate((-(origin[0] as f32), -(origin[1] as f32)));
            let drawn = draw(canvas);
            // Snap also after a failed drawing, so the recorder holds nothing of it.
            let mut recording = self.recorder.snap().context("record Skia drawing")?;
            drawn?;
            self.context
                .insert_recording(&graphite::InsertRecordingInfo::new(&mut recording));
            ensure!(self.context.submit(None), "submit Skia drawing");
            self.context.check_async_work_completion();
            Ok((texture, origin))
        }
    }
}

#[cfg(feature = "skia-gpu")]
pub(crate) use graphite::SkiaGpu;

/// A build without the painter has no value of this type, so its call sites need no conditions.
#[cfg(not(feature = "skia-gpu"))]
pub(crate) enum SkiaGpu {}

#[cfg(not(feature = "skia-gpu"))]
impl SkiaGpu {
    pub(crate) fn new(_: &wgpu::Device, _: &wgpu::Queue) -> Result<Option<Self>> {
        Ok(None)
    }
}

#[cfg(not(feature = "skia-gpu"))]
impl Painter for SkiaGpu {
    type Picture = Texture;
    fn paint(
        &mut self,
        _: [u32; 2],
        _: [i32; 2],
        _: impl FnOnce(&Canvas) -> Result<()>,
    ) -> Result<Texture> {
        match *self {}
    }
}

/// Bytes that differ by more than a rounding step. A native path is drawn on the device and an
/// SVG of the same shape on the CPU, so their antialiased edges may round differently.
#[cfg(test)]
pub(crate) fn bytes_beyond_edge_rounding(left: &[u8], right: &[u8]) -> usize {
    assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .filter(|(a, b)| a.abs_diff(**b) > 1)
        .count()
}
