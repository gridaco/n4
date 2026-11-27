//! Convolve an isolated drawable on its native pixel grid. Resolve the result
//! against the sharp source at paint time so camera and band edits need no blur.
use super::{
    Gpu, ScenePainter, ShaderKind, raster_bounds,
    sampled::{
        MAX_SCRATCH_BYTES, Pass, Region, gaussian_weights, linear_surface, sampling_uniform,
    },
    source_effects::stage_count,
};
use crate::{
    Project,
    graphics::{Content, Graphic, PaintCache, SourceEffect},
};
use anyhow::{Result, ensure};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

pub(super) const GAUSSIAN: &str = include_str!("gaussian.wgsl");

pub(super) enum SourceKey {
    Raster(wgpu::Texture),
    Decoded(Weak<Vec<u8>>, [u32; 2]),
}

impl PartialEq for SourceKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Raster(a), Self::Raster(b)) => a == b,
            (Self::Decoded(a, size), Self::Decoded(b, size_b)) => size == size_b && a.ptr_eq(b),
            _ => false,
        }
    }
}

struct Entry {
    source: SourceKey,
    sigma: f32,
    blurred: wgpu::Texture,
    bytes: u64,
}

#[derive(Default)]
pub(super) struct Cache {
    rows: Option<wgpu::Texture>,
    results: VecDeque<Entry>,
    pub(super) stages: [Option<wgpu::Texture>; 2],
    transient_result: Option<wgpu::Texture>,
}

impl Cache {
    fn trim_results(&mut self, reserved: u64, max_entries: usize) {
        let mut retained = self.results.iter().map(|entry| entry.bytes).sum::<u64>();
        while !self.results.is_empty()
            && (retained + reserved > MAX_SCRATCH_BYTES || self.results.len() > max_entries)
        {
            retained -= self.results.pop_front().expect("occupied blur cache").bytes;
        }
    }

    pub(super) fn reserve_stages(
        &mut self,
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        count: usize,
    ) -> Result<()> {
        workspace([size.width, size.height], count)?;
        // Release differently sized scratch before admitting a larger stack.
        for slot in self.stages.iter_mut().skip(count) {
            *slot = None;
        }
        for slot in self
            .stages
            .iter_mut()
            .chain([&mut self.rows, &mut self.transient_result])
        {
            if slot.as_ref().is_some_and(|t| t.size() != size) {
                *slot = None;
            }
        }
        let reserved_surfaces =
            count + usize::from(self.rows.is_some()) + usize::from(self.transient_result.is_some());
        self.trim_results(
            u64::from(size.width) * u64::from(size.height) * 8 * reserved_surfaces as u64,
            8,
        );
        for slot in &mut self.stages[..count] {
            slot.get_or_insert_with(|| linear_surface(device, "ordered source effects", size));
        }
        Ok(())
    }

    pub(super) fn clear_results(&mut self) {
        self.results.clear();
    }
}

fn workspace([w, h]: [u32; 2], stages: usize) -> Result<u64> {
    // Horizontal and vertical RGBA16F surfaces. For encoded paint, reuse the
    // vertical output for the initial conversion before either Gaussian pass.
    let bytes = (u64::from(w) * u64::from(h))
        .checked_mul(8 * (2 + stages as u64))
        .ok_or_else(|| anyhow::anyhow!("source blur workspace exceeds addressable byte range"))?;
    ensure!(
        bytes <= MAX_SCRATCH_BYTES,
        "source blur needs {bytes} private texture bytes, exceeding the 256 MiB limit"
    );
    Ok(bytes)
}

impl Gpu {
    pub(crate) fn validate_source_blur_scratch(
        project: &Project,
        cache: &PaintCache,
    ) -> Result<()> {
        fn visit(layers: &[Graphic], size: [u32; 2], cache: &PaintCache) -> Result<()> {
            for layer in layers {
                if let Content::Group { children } | Content::TransformGroup { children } =
                    &layer.content
                {
                    visit(children, size, cache)?;
                } else if let Content::Scene3d(scene) = &layer.content {
                    for surface in scene.models.iter().flat_map(|m| &m.surfaces) {
                        visit(&surface.layers, surface.size, cache)?;
                    }
                }
                let mut spatial = Vec::new();
                for effect in &layer.effects {
                    match effect {
                        SourceEffect::SourceBlur(blur)
                            if blur.sigma.range("source blur sigma")?[1] > 0.
                                && blur
                                    .amount
                                    .as_ref()
                                    .map(|a| a.range("source blur amount"))
                                    .transpose()?
                                    .is_none_or(|r| r[1] > 0.) =>
                        {
                            spatial.push(true)
                        }
                        SourceEffect::RgbGrade(grade) if !grade.always_identity() => {
                            spatial.push(false)
                        }
                        _ => {}
                    }
                }
                let Some(stages) = stage_count(spatial.into_iter()) else {
                    continue;
                };
                let size = match &layer.content {
                    Content::Footage { .. } | Content::Scene3d(_) => size,
                    Content::Video(video) => video.size,
                    Content::Rect(rect) => rect.raster_envelope()?.max_size,
                    Content::Path(path) => super::super::path::raster_envelope(path)?.max_size,
                    _ if !layer.text_animators.is_empty() => cache.text_offset_bounds(layer)?.1,
                    content => raster_bounds(content, cache)?.1,
                };
                workspace(size, stages)?;
                if let Some(cues) = &layer.type_on {
                    for (frame, _) in cues {
                        if let Some(prefix) = layer.content_at(*frame, 0.) {
                            workspace(raster_bounds(&prefix, cache)?.1, stages)?;
                        }
                    }
                }
            }
            Ok(())
        }
        for scope in project.graphic_scopes() {
            visit(scope, project.size, cache)?;
        }
        Ok(())
    }
}

impl ScenePainter<'_> {
    pub(super) fn source_key(&self, source: &wgpu::Texture, content: &Content) -> SourceKey {
        let picture = match content {
            Content::Footage { history: None } => self.video_pixels,
            Content::Video(clip) => clip.source_frame(self.time.frame).and_then(|tick| {
                self.video_layer_pixels
                    .get(&(clip.source.as_path(), clip.size, tick))
            }),
            _ => None,
        };
        // The decoder can replace a frame at the same path and ordinal. A weak
        // pixel identity survives held frames without pinning the decoder LRU.
        picture.map_or_else(
            || SourceKey::Raster(source.clone()),
            |buffer| SourceKey::Decoded(Arc::downgrade(buffer), [source.width(), source.height()]),
        )
    }

    pub(super) fn blur_source(
        &mut self,
        source: &wgpu::Texture,
        key: Option<SourceKey>,
        encoded: bool,
        sigma: f32,
    ) -> Result<wgpu::Texture> {
        let mut cache = self.gpu.source_blurs.borrow_mut();
        if let Some(i) = cache
            .results
            .iter()
            .position(|entry| Some(&entry.source) == key.as_ref() && entry.sigma == sigma)
        {
            let hit = cache.results.remove(i).expect("matched a cached blur");
            let blurred = hit.blurred.clone();
            cache.results.push_back(hit);
            self.stats.source_blur_cache_hits += 1;
            return Ok(blurred);
        }

        let region = Region {
            x: 0,
            y: 0,
            width: source.width(),
            height: source.height(),
        };
        let stages = cache.stages.iter().flatten().count();
        let working_bytes = workspace([region.width, region.height], stages)?;
        let result_bytes = region.pixels() * 8;
        let pinned_source_bytes = if matches!(key, Some(SourceKey::Raster(_))) {
            region.pixels() * 4
        } else {
            0
        };
        // A transient result may be overwritten only after its prior consumer
        // has been encoded. Cached results each own an immutable texture.
        let cache_result =
            key.is_some() && working_bytes + pinned_source_bytes <= MAX_SCRATCH_BYTES;
        cache.trim_results(
            working_bytes + if cache_result { pinned_source_bytes } else { 0 },
            if cache_result { 7 } else { 8 },
        );
        let blurred = cache
            .transient_result
            .take()
            .unwrap_or_else(|| linear_surface(&self.gpu.device, "blurred source", region.extent()));
        let rows = cache.rows.get_or_insert_with(|| {
            linear_surface(
                &self.gpu.device,
                "source blur horizontal rows",
                region.extent(),
            )
        });
        let gaussian = self.gpu.effect(ShaderKind::Sampled, GAUSSIAN)?;
        let radius = (3. * sigma).ceil() as u32;
        let weights = gaussian_weights(sigma, radius);
        if encoded {
            let convert = self.gpu.effect(ShaderKind::LinearSource, "")?;
            self.filter_pass(Pass {
                label: "unassociate and linearize source paint",
                inputs: &[source],
                output: &blurred,
                viewport: region,
                pipeline: &convert,
                uniform: &[0; 8],
                preserve: false,
            });
        }
        self.filter_pass(Pass {
            label: "source blur along X",
            inputs: &[if encoded { &blurred } else { source }],
            output: rows,
            viewport: region,
            pipeline: &gaussian,
            uniform: &sampling_uniform(region, region, [1, 0], radius, &weights, false),
            preserve: false,
        });
        self.filter_pass(Pass {
            label: "source blur along Y",
            inputs: &[rows],
            output: &blurred,
            viewport: region,
            pipeline: &gaussian,
            uniform: &sampling_uniform(region, region, [0, 1], radius, &weights, false),
            preserve: false,
        });
        if cache_result {
            let key = key.expect("an immutable input is eligible for caching");
            cache.results.push_back(Entry {
                source: key,
                sigma,
                blurred: blurred.clone(),
                bytes: result_bytes + pinned_source_bytes,
            });
        } else {
            cache.transient_result = Some(blurred.clone());
        }
        self.stats.source_blur_passes += 2;
        self.stats.source_blur_filtered_pixels += region.pixels() * 2;
        self.stats.source_blur_scratch_bytes =
            self.stats.source_blur_scratch_bytes.max(working_bytes);
        Ok(blurred)
    }
}
