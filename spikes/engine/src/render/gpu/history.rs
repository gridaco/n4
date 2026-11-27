//! Map project-frame ages to resident decoded pictures and run a project WGSL
//! pass in footage-local pixels. Every render supplies its full input window,
//! so a cold seek and sequential playback evaluate the same source pictures.
use super::sampled::{MAX_SCRATCH_BYTES, Pass, Region, linear_surface};
use super::{Gpu, ScenePainter, ShaderKind, TARGET_FORMAT};
use crate::graphics::{MAX_SOURCE_HISTORY_FRAMES, SourceHistory};
use anyhow::{Context, Result, ensure};
use std::sync::{Arc, Weak};

// One uniform vec4 for the bounds and four vec4s for logical-age lookup.
const SLOT_WORDS: usize = 16;
const _: () = assert!(MAX_SOURCE_HISTORY_FRAMES < SLOT_WORDS as u32);
const MISSING: u32 = u32::MAX;

pub(super) fn check_budget([w, h]: [u32; 2], previous: u32) -> Result<()> {
    ensure!(
        (1..=MAX_SOURCE_HISTORY_FRAMES).contains(&previous),
        "source history requires 1 to {MAX_SOURCE_HISTORY_FRAMES} previous frames"
    );
    let bytes = (u64::from(w) * u64::from(h))
        .checked_mul(4 * (u64::from(previous) + 1) + 8)
        .context("source-history working set overflow")?;
    ensure!(
        bytes <= MAX_SCRATCH_BYTES,
        "source history needs {bytes} GPU bytes for decoded frames and output, exceeding the 256 MiB limit"
    );
    Ok(())
}

pub(super) struct Atlas {
    frames: wgpu::Texture,
    output: wgpu::Texture,
    // Weak identities allow the decoder to reclaim the original CPU buffers.
    residents: Vec<Weak<Vec<u8>>>,
}

impl Atlas {
    fn slot(&self, frame: &Arc<Vec<u8>>) -> Option<usize> {
        self.residents
            .iter()
            .position(|entry| entry.as_ptr() == Arc::as_ptr(frame))
    }
}

pub(super) struct Window {
    atlas: wgpu::Texture,
    output: wgpu::Texture,
    slots: [u32; SLOT_WORDS],
    previous: u32,
    pub(super) uploads: u32,
}

impl Gpu {
    pub(super) fn prepare_history(
        &self,
        commands: &mut wgpu::CommandEncoder,
        current: &wgpu::Texture,
        pixels: &Arc<Vec<u8>>,
        previous: &[Option<Arc<Vec<u8>>>],
    ) -> Result<Window> {
        let count = u32::try_from(previous.len())?;
        check_budget([current.width(), current.height()], count)?;
        let size = wgpu::Extent3d {
            depth_or_array_layers: count + 1,
            ..current.size()
        };
        let mut cache = self.source_history.borrow_mut();
        if cache
            .as_ref()
            .is_none_or(|atlas| atlas.frames.size() != size)
        {
            *cache = Some(Atlas {
                frames: self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("decoded source history"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: TARGET_FORMAT,
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                }),
                output: linear_surface(&self.device, "temporal source result", current.size()),
                residents: vec![Weak::new(); previous.len() + 1],
            });
        }
        let atlas = cache.as_mut().expect("source history allocated");
        let frames = std::iter::once(Some(pixels)).chain(previous.iter().map(Option::as_ref));
        let mut slots = [MISSING; SLOT_WORDS];
        let mut pinned = [false; SLOT_WORDS];
        // Reserve every hit first, including those needed only at the oldest age.
        for frame in frames.clone().flatten() {
            if let Some(slot) = atlas.slot(frame) {
                pinned[slot] = true;
            }
        }
        let mut uploads = 0;
        for (age, frame) in frames.enumerate() {
            let Some(frame) = frame else { continue };
            // A held source frame can also occupy several logical ages.
            if let Some(slot) = atlas.slot(frame) {
                slots[age] = slot as u32;
                continue;
            }
            let slot = pinned[..atlas.residents.len()]
                .iter()
                .position(|&used| !used)
                .context("no free source-history array layer")?;
            pinned[slot] = true;
            slots[age] = slot as u32;
            atlas.residents[slot] = Arc::downgrade(frame);
            let dest = wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d {
                    z: slot as u32,
                    ..Default::default()
                },
                ..atlas.frames.as_image_copy()
            };
            if age == 0 {
                // The normal primary upload already transferred this frame.
                commands.copy_texture_to_texture(current.as_image_copy(), dest, current.size());
            } else {
                ensure!(
                    frame.len() as u64 == u64::from(size.width) * u64::from(size.height) * 4,
                    "wrong RGBA8 byte count for source-history frame"
                );
                self.queue.write_texture(
                    dest,
                    frame,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size.width * 4),
                        rows_per_image: None,
                    },
                    current.size(),
                );
                uploads += 1;
            }
        }
        Ok(Window {
            atlas: atlas.frames.clone(),
            output: atlas.output.clone(),
            slots,
            previous: count,
            uploads,
        })
    }
}

impl ScenePainter<'_> {
    pub(super) fn temporal_source(&mut self, recipe: &SourceHistory) -> Result<wgpu::Texture> {
        let window = self
            .history
            .context("missing source-history inputs for temporal footage")?;
        ensure!(
            recipe.frames <= window.previous,
            "missing declared source-history frames"
        );
        let pipeline = self
            .gpu
            .effect(ShaderKind::Temporal, self.cache.shader(&recipe.shader)?)?;
        let uniform: Vec<_> = [recipe.frames, 0, 0, 0]
            .into_iter()
            .chain(window.slots)
            .flat_map(u32::to_le_bytes)
            .collect();
        self.filter_pass(Pass {
            label: "source history",
            inputs: &[&window.atlas],
            output: &window.output,
            viewport: Region {
                x: 0,
                y: 0,
                width: window.output.width(),
                height: window.output.height(),
            },
            pipeline: &pipeline,
            uniform: &uniform,
            preserve: false,
        });
        self.stats.history_passes += 1;
        Ok(window.output.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{Graphic, PaintCache};
    use crate::render::gpu::{FrameInputs, PrimaryPicture};
    use std::{collections::HashMap, fs};

    #[test]
    fn history_budget_accounts_for_the_current_picture_and_linear_output() {
        assert!(check_budget([1920, 1080], 12).is_ok());
        assert!(check_budget([3840, 2160], 5).is_ok());
        assert!(check_budget([3840, 2160], 6).is_err());
    }

    #[test]
    fn temporal_shader_reads_declared_source_frames_in_any_seek_order() -> Result<()> {
        let gpu = Gpu::new()?;
        let dir = tempfile::tempdir()?;
        let shader = dir.path().join("ages.wgsl");
        fs::write(
            &shader,
            r#"
            fn temporal(pixel: vec2<i32>, current: vec4<f32>, previous_frames: u32) -> vec4<f32> {
                if pixel.x == 0 { return current; }
                if pixel.x == 3 { return history_at(vec2<i32>(-1, 0), 1u); }
                if pixel.x == 4 { return history_at(pixel, previous_frames + 1u); }
                return history_at(pixel, u32(pixel.x));
            }
        "#,
        )?;
        let layer: Graphic = serde_json::from_value(serde_json::json!({
            "content": {"type": "footage", "history": {"shader": shader, "frames": 2}},
            "span": [10, 100],
        }))?;
        let size = [5, 1];
        let mut cache = PaintCache::default();
        gpu.validate_effects(std::slice::from_ref(&layer), &mut cache, size)?;
        let make_frame = |level| Arc::new([level, 0, 0, 255].repeat(5));
        let (a, b, c, d) = (
            make_frame(60),
            make_frame(120),
            make_frame(180),
            make_frame(240),
        );
        let render = |current, previous: [Option<Arc<Vec<u8>>>; 3], cache: &mut PaintCache| {
            gpu.compose_view(
                FrameInputs {
                    size,
                    primary: PrimaryPicture::DecodedHistory(current, &previous),
                    video_layers: &HashMap::new(),
                    overlay: None,
                    scopes: vec![(std::slice::from_ref(&layer), 10)],
                },
                None,
                None,
                cache,
            )
        };
        let red = |pixels: &[u8]| pixels.chunks_exact(4).map(|p| p[0]).collect::<Vec<_>>();
        // The layer just became visible, but the underlying source has earlier pictures.
        let cold = render(&c, [Some(b.clone()), Some(a.clone()), None], &mut cache)?;
        assert_eq!(red(&cold.0), [180, 120, 60, 0, 0]);
        assert_eq!((cold.1.history_uploads, cold.1.history_passes), (2, 1));
        let next = render(
            &d,
            [Some(c.clone()), Some(b.clone()), Some(a.clone())],
            &mut cache,
        )?;
        assert_eq!(red(&next.0), [240, 180, 120, 0, 0]);
        assert_eq!(next.1.history_uploads, 0);
        let first = render(&a, [None, None, None], &mut cache)?;
        assert_eq!(red(&first.0), [60, 0, 0, 0, 0]);
        let revisit = render(&c, [Some(b.clone()), Some(a.clone()), None], &mut cache)?;
        assert_eq!(cold.0, revisit.0);
        let held = render(&c, [Some(c.clone()), Some(b.clone()), None], &mut cache)?;
        assert_eq!(red(&held.0), [180, 180, 120, 0, 0]);
        assert_eq!(held.1.history_uploads, 0);
        Ok(())
    }
}
