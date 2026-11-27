//! Draw egui meshes into the player's existing accelerated SDL renderer.
//! All textures use the ordinary rust-sdl2 lifetime and Drop semantics.
use anyhow::{Context, Result, bail};
use egui::{ClippedPrimitive, TextureId, TexturesDelta, epaint::Primitive};
use sdl2::{
    pixels::{Color, PixelFormatEnum},
    rect::{FPoint, Rect},
    render::{BlendMode, ScaleMode, Texture, TextureCreator, Vertex, WindowCanvas},
    video::WindowContext,
};
use std::collections::HashMap;

pub struct Painter<'a> {
    creator: &'a TextureCreator<WindowContext>,
    textures: HashMap<TextureId, Texture<'a>>,
    vertices: Vec<Vertex>,
}

impl<'a> Painter<'a> {
    pub fn new(creator: &'a TextureCreator<WindowContext>) -> Self {
        Self {
            creator,
            textures: HashMap::new(),
            vertices: Vec::new(),
        }
    }

    pub fn paint(
        &mut self,
        canvas: &mut WindowCanvas,
        jobs: &[ClippedPrimitive],
        delta: TexturesDelta,
        scale: f32,
    ) -> Result<()> {
        for (id, change) in delta.set {
            let egui::ImageData::Color(image) = change.image;
            let [w, h] = image.size.map(|v| v as u32);
            let region = if let Some([x, y]) = change.pos {
                Some(Rect::new(x as i32, y as i32, w, h))
            } else {
                let mut texture =
                    self.creator
                        .create_texture_static(PixelFormatEnum::RGBA32, w, h)?;
                texture.set_blend_mode(BlendMode::Blend);
                self.textures.insert(id, texture);
                None
            };
            let texture = self
                .textures
                .get_mut(&id)
                .context("missing inspector texture")?;
            texture.set_scale_mode(match change.options.magnification {
                egui::TextureFilter::Nearest => ScaleMode::Nearest,
                egui::TextureFilter::Linear => ScaleMode::Linear,
            });
            let pixels: Vec<_> = image
                .pixels
                .iter()
                .flat_map(|p| p.to_srgba_unmultiplied())
                .collect();
            texture.update(region, &pixels, w as usize * 4)?;
        }
        let previous_clip = canvas.clip_rect();
        let size = canvas.output_size().map_err(anyhow::Error::msg)?;
        for job in jobs {
            let Primitive::Mesh(mesh) = &job.primitive else {
                bail!("unsupported inspector paint callback")
            };
            let clip = job.clip_rect;
            let x = (clip.min.x * scale).round().clamp(0., size.0 as f32) as i32;
            let y = (clip.min.y * scale).round().clamp(0., size.1 as f32) as i32;
            let right = (clip.max.x * scale).round().clamp(0., size.0 as f32) as i32;
            let bottom = (clip.max.y * scale).round().clamp(0., size.1 as f32) as i32;
            if x >= right || y >= bottom || mesh.indices.is_empty() {
                continue;
            }
            canvas.set_clip_rect(Rect::new(x, y, (right - x) as u32, (bottom - y) as u32));
            let texture = self
                .textures
                .get(&mesh.texture_id)
                .context("unknown inspector texture")?;
            self.vertices.clear();
            self.vertices.extend(mesh.vertices.iter().map(|v| {
                let [r, g, b, a] = v.color.to_srgba_unmultiplied();
                Vertex {
                    position: FPoint::new(v.pos.x * scale, v.pos.y * scale),
                    tex_coord: FPoint::new(v.uv.x, v.uv.y),
                    color: Color::RGBA(r, g, b, a),
                }
            }));
            canvas
                .render_geometry(&self.vertices, Some(texture), mesh.indices.as_slice())
                .map_err(anyhow::Error::msg)?;
        }
        canvas.set_clip_rect(previous_clip);
        for id in delta.free {
            self.textures.remove(&id);
        }
        Ok(())
    }
}
