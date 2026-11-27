use super::Gpu;
use crate::project::Background;
use anyhow::{Result, ensure};
use wgpu::util::DeviceExt;

/// A single stationary plate, kept pristine so drawing and interrupted seeks
/// cannot write back into the background shown on the next frame.
pub(super) struct Cached {
    material: Background,
    texture: wgpu::Texture,
}

impl Gpu {
    pub(super) fn background_texture(
        &self,
        [width, height]: [u32; 2],
        material: &Background,
    ) -> Result<wgpu::Texture> {
        if let Some(cached) = self.background.borrow().as_ref()
            && &cached.material == material
            && cached.texture.width() == width
            && cached.texture.height() == height
        {
            return Ok(cached.texture.clone());
        }
        let limit = self.device.limits().max_texture_dimension_2d;
        ensure!(
            (1..=limit).contains(&width) && (1..=limit).contains(&height),
            "background extent {width} × {height} exceeds the supported range 1..={limit}"
        );
        let texture = self.render_texture(
            "native background",
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let color =
            u32::from_le_bytes([material.color[0], material.color[1], material.color[2], 0]);
        let values = [color, u32::from(material.grain), material.seed, 0];
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("background material"),
                contents: &values.map(u32::to_le_bytes).concat(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("background material"),
            layout: &self.background_pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut commands = self.device.create_command_encoder(&Default::default());
        let view = texture.create_view(&Default::default());
        {
            let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paint native background"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.background_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        // A later compose can fail before its encoder submits. Publish only an
        // initialized plate, ordered on the same queue as subsequent paint.
        self.queue.submit(Some(commands.finish()));
        *self.background.borrow_mut() = Some(Cached {
            material: *material,
            texture: texture.clone(),
        });
        Ok(texture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graphics::{Exposure, Graphic, PaintCache},
        render::gpu::{FrameInputs, PrimaryPicture},
    };
    use serde_json::json;

    #[test]
    fn background_is_editable_and_pristine_across_frames_and_exposures() -> Result<()> {
        let gpu = Gpu::new()?;
        let background = Background {
            color: [30, 95, 170],
            grain: 0,
            seed: 0,
        };
        let draw = |mat: &Background,
                    shot: &[Graphic],
                    global: &[Graphic],
                    frame: u32,
                    exposure: Option<Exposure>|
         -> Result<Vec<u8>> {
            Ok(gpu
                .compose_exposed(
                    FrameInputs {
                        size: [31, 19],
                        primary: PrimaryPicture::Background(mat),
                        video_layers: &Default::default(),
                        overlay: None,
                        scopes: vec![(shot, frame - 7), (global, frame)],
                    },
                    exposure,
                    &mut PaintCache::default(),
                )?
                .0)
        };
        let plain = draw(&background, &[], &[], 7, None)?;
        assert!(plain.chunks_exact(4).all(|p| p == [30, 95, 170, 255]));
        let grained = Background {
            grain: 27,
            seed: 3,
            ..background
        };
        let grain = draw(&grained, &[], &[], 8, None)?;
        assert_ne!(grain, plain);
        assert_ne!(
            grain,
            draw(&Background { seed: 4, ..grained }, &[], &[], 8, None)?
        );

        let shot: Vec<Graphic> = serde_json::from_value(json!([
            {"content":{"type":"footage"}},
            {"span":[1,2], "content":{"type":"group","children":[
                {"content":{"type":"path","d":"M0 0H20V19H0Z","fill":[250,3,90,255]}}
            ]}}
        ]))?;
        let global: Vec<Graphic> = serde_json::from_value(json!([
            {"span":[8,9], "content":{"type":"path","d":"M10 0H25V19H10Z","fill":[15,190,45,255]}}
        ]))?;
        let result = draw(
            &grained,
            &shot,
            &global,
            8,
            Some(Exposure {
                samples: 4,
                angle_degrees: 180,
            }),
        )?;
        let pixel = |x: usize| &result[x * 4..x * 4 + 4];
        assert_eq!(pixel(2), [250, 3, 90, 255]);
        assert_eq!(pixel(15), [15, 190, 45, 255]);
        assert_eq!(pixel(28), &grain[28 * 4..28 * 4 + 4]);
        assert_eq!(grain, draw(&grained, &shot, &global, 7, None)?);
        assert_eq!(plain, draw(&background, &[], &[], 7, None)?);
        Ok(())
    }
}
