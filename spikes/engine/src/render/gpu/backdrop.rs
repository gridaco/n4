//! Backdrop sampling and masked replacement at paint order.
//! Positions passed to shaders stay in scope coordinates as scratch textures crop.
use super::{
    Bounds, Gpu, ScenePainter, ShaderKind, ancestor_envelope,
    sampled::{
        MAX_SCRATCH_BYTES, Pass, Region, Scratch, gaussian_weights, sampling_uniform, scratch_bytes,
    },
    translated,
};
use crate::{
    Project,
    graphics::{BackdropFilter, Content, Graphic, PaintCache, SampleTime},
    render::{affine::SampledTransform, path, svg},
};
use anyhow::{Context, Result};

pub(super) const STATIC_COVERAGE: &str =
    "fn coverage_amount(local: vec2<f32>, phase: f32, alpha: f32) -> f32 { return alpha; }";

pub(super) fn shader_kind(filter: &BackdropFilter) -> ShaderKind {
    match filter {
        BackdropFilter::Separable(_) => ShaderKind::Sampled,
        BackdropFilter::Sampled2d { .. } => ShaderKind::Backdrop2d,
    }
}

#[derive(Clone, Copy)]
struct WorkspaceLayout {
    roi: Region,
    rows: Region,
    radius: u32,
}

impl WorkspaceLayout {
    /// Visible writes and intermediate rows share exactly this calculation at
    /// edit admission and render time, including offscreen origins and wipes.
    fn clipped(scope: [u32; 2], bounds: Bounds, filter: &BackdropFilter) -> Option<Self> {
        let bounds = bounds.intersect(Bounds::new([0; 2], scope));
        if bounds.empty() {
            return None;
        }
        let roi = Region {
            x: bounds.left as u32,
            y: bounds.top as u32,
            width: bounds.width(),
            height: bounds.height(),
        };
        let radius = match filter {
            BackdropFilter::Sampled2d {
                sample_radius_px, ..
            } => {
                // The 2D pass reads the immutable target directly. It never needs
                // horizontal rows; reserve the minimum legal intermediate texture.
                return Some(Self {
                    roi,
                    rows: Region {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                    radius: *sample_radius_px,
                });
            }
            BackdropFilter::Separable(s) if s.sigma_px.get() == 0.0 => return None,
            BackdropFilter::Separable(s) => (3.0 * s.sigma_px.get()).ceil() as u32,
        };
        // H reads the original on X. V needs H rows beyond the card on Y.
        let first = roi.y.saturating_sub(radius);
        let last = (roi.y + roi.height + radius).min(scope[1]);
        let rows = Region {
            y: first,
            height: last - first,
            ..roi
        };
        Some(Self { roi, rows, radius })
    }
    fn checked_bytes(self) -> Result<u64> {
        scratch_bytes([self.roi, self.rows, self.roi])
    }
}

impl Gpu {
    pub(crate) fn validate_backdrop_scratch(project: &Project, cache: &PaintCache) -> Result<()> {
        let last_sample = project.exposure.map_or(0, |e| u32::from(e.samples) - 1);
        project.visit_framed_scopes(|graphics, duration, size, view| {
            let [w, h] = size;
            if u64::from(w) * u64::from(h) * 20 <= MAX_SCRATCH_BYTES {
                return Ok(());
            }
            let end = duration.checked_sub(1).context("empty backdrop scope")?;
            let time = SampleTime::at(end, project.exposure, last_sample);
            validate_layers(size, graphics, [0; 2], time, cache, &mut Vec::new(), view)
        })
    }
}

fn validate_layers<'a>(
    scope: [u32; 2],
    layers: &'a [Graphic],
    parent: [i64; 2],
    time: SampleTime,
    cache: &PaintCache,
    ancestors: &mut Vec<([i64; 2], &'a Graphic)>,
    view: Option<crate::camera::OrthoRanges>,
) -> Result<()> {
    for layer in layers {
        let origin = translated(parent, layer.placed_position());
        match &layer.content {
            Content::Group { children } | Content::TransformGroup { children } => {
                let transformed = layer.has_spatial_transform();
                if transformed {
                    ancestors.push((origin, layer));
                }
                validate_layers(scope, children, origin, time, cache, ancestors, view)?;
                if transformed {
                    ancestors.pop();
                }
            }
            Content::Backdrop { filter } => {
                if matches!(filter, BackdropFilter::Separable(s) if s.sigma_px.get() == 0.) {
                    continue;
                }
                let mask = layer.mask.as_ref().context("backdrop requires a mask")?;
                let content = mask.as_content();
                // Accepted SVG rasters remain pinned after an external file edit.
                let (crop, [width, height]) = match cache.cached_raster_bounds(&content) {
                    Some(bounds) => bounds,
                    None => match content {
                        Content::Rect(rect) => rect.raster_bounds()?,
                        Content::Path(native) => path::raster_bounds(&native)?,
                        Content::Svg { source } => ([0; 2], svg::raster_size(&source)?),
                        _ => anyhow::bail!("unsupported backdrop mask"),
                    },
                };
                let origin = translated(translated(origin, mask.position), crop);
                // A moving mask starts each wipe at its sampled raster origin.
                let visible = if mask.animated_rect().is_some() {
                    width
                } else {
                    layer.wipe.map_or(width, |range| time.reveal(range, width))
                };
                let bounds = ancestor_envelope(Bounds::new(origin, [visible, height]), ancestors)
                    .through_camera(scope, view);
                if let Some(layout) = WorkspaceLayout::clipped(scope, bounds, filter) {
                    layout.checked_bytes()?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

impl ScenePainter<'_> {
    pub(super) fn backdrop(
        &mut self,
        layer: &Graphic,
        filter: &BackdropFilter,
        target: &wgpu::Texture,
        origin: [i64; 2],
        transform: Option<SampledTransform>,
        frame: u32,
    ) -> Result<()> {
        if matches!(filter, BackdropFilter::Separable(s) if s.sigma_px.get() == 0.) {
            return Ok(());
        }
        let mask = layer.mask.as_ref().context("backdrop requires a mask")?;
        let (silhouette, crop) = self.mask_texture(mask, frame)?;
        let local_origin = translated(origin, mask.position);
        let texture_origin = translated(local_origin, crop);
        let visible_width = layer.wipe.map_or(silhouette.width(), |range| {
            self.time.reveal(range, silhouette.width())
        });
        let visible = Bounds::new(texture_origin, [visible_width, silhouette.height()]);
        let Some(layout) = WorkspaceLayout::clipped(
            [target.width(), target.height()],
            visible.transformed(origin, transform),
            filter,
        ) else {
            return Ok(());
        };
        let WorkspaceLayout { roi, rows, radius } = layout;
        let scope = Region {
            x: 0,
            y: 0,
            width: target.width(),
            height: target.height(),
        };
        let sampled = self
            .gpu
            .effect(shader_kind(filter), self.cache.shader(filter.shader())?)?;
        let (coverage_shader, phase) = match &mask.shader {
            Some(shader) => (
                self.cache.shader(&shader.shader)?,
                self.time.phase(shader.period_frames),
            ),
            None => (STATIC_COVERAGE, 0.0),
        };
        let resolve = self.gpu.effect(ShaderKind::Coverage, coverage_shader)?;
        let mut workspace = self.gpu.sampled_scratch.borrow_mut();
        let scratch = Scratch::reserve(&mut workspace, &self.gpu.device, [roi, rows, roi])?;
        // Capture the unfiltered ROI for replacement through fractional alpha.
        // The sampling pass reads the untouched render target in a distinct pass.
        let mut src = target.as_image_copy();
        src.origin = wgpu::Origin3d {
            x: roi.x,
            y: roi.y,
            z: 0,
        };
        self.commands
            .copy_texture_to_texture(src, scratch.original.as_image_copy(), roi.extent());

        let transform = transform.unwrap_or(SampledTransform::IDENTITY);
        let [dx, dy] = transform.map_point(mask.position.map(f64::from));
        let mask_origin = [origin[0] as f64 + dx, origin[1] as f64 + dy];
        match filter {
            BackdropFilter::Separable(s) => {
                let weights = gaussian_weights(s.sigma_px.get(), radius);
                self.filter_pass(Pass {
                    label: "horizontal sampled filter",
                    inputs: &[target],
                    output: &scratch.horizontal,
                    viewport: Region { x: 0, y: 0, ..rows },
                    pipeline: &sampled,
                    uniform: &sampling_uniform(scope, rows, [1, 0], radius, &weights, false),
                    preserve: false,
                });
                self.filter_pass(Pass {
                    label: "vertical sampled filter",
                    inputs: &[&scratch.horizontal],
                    output: &scratch.vertical,
                    viewport: Region { x: 0, y: 0, ..roi },
                    pipeline: &sampled,
                    uniform: &sampling_uniform(rows, roi, [0, 1], radius, &weights, false),
                    preserve: false,
                });
                self.stats.backdrop_passes += 2;
                self.stats.backdrop_filtered_pixels += rows.pixels() + roi.pixels();
            }
            BackdropFilter::Sampled2d { params, .. } => {
                let header = [
                    roi.x,
                    roi.y,
                    radius,
                    0,
                    silhouette.width(),
                    silhouette.height(),
                    0,
                    0,
                ];
                let mut values = [0.0f32; 4];
                for (slot, param) in values.iter_mut().zip(params) {
                    *slot = param.value.at(frame, self.time.fraction()).into();
                }
                let uniform: Vec<u8> = header
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .chain(
                        mask_origin
                            .into_iter()
                            .chain(crop.map(f64::from))
                            .chain(transform.inverse_rows().into_iter().flatten())
                            .flat_map(|v| (v as f32).to_le_bytes()),
                    )
                    .chain(values.into_iter().flat_map(f32::to_le_bytes))
                    .collect();
                self.filter_pass(Pass {
                    label: "sample 2D backdrop",
                    inputs: &[target],
                    output: &scratch.vertical,
                    viewport: Region { x: 0, y: 0, ..roi },
                    pipeline: &sampled,
                    uniform: &uniform,
                    preserve: false,
                });
                self.stats.backdrop_passes += 1;
                self.stats.backdrop_filtered_pixels += roi.pixels();
            }
        }
        // The filtered surface is in output pixels. Pull coverage back into
        // the mask's authored coordinates for the final replacement.
        let mut placement: Vec<_> = [roi.x as i32, roi.y as i32, crop[0], crop[1]]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect();
        placement.extend(
            mask_origin
                .into_iter()
                .chain(transform.inverse_rows().into_iter().flatten())
                .flat_map(|v| (v as f32).to_le_bytes()),
        );
        placement.extend(phase.to_le_bytes());
        placement.extend((visible_width as i32).to_le_bytes());
        self.filter_pass(Pass {
            label: "replace backdrop through coverage",
            inputs: &[&scratch.original, &scratch.vertical, &silhouette],
            output: target,
            viewport: roi,
            pipeline: &resolve,
            uniform: &placement,
            preserve: true,
        });
        // Encoded work per output frame, summed across adjustments/exposure samples.
        // Scratch is the largest retained workspace used by an enabled pass in this
        // frame; bypassed frames report zero even if a previous allocation is cached.
        self.stats.backdrop_passes += 1;
        self.stats.backdrop_copy_bytes += roi.pixels() * 4;
        self.stats.backdrop_scratch_bytes = self.stats.backdrop_scratch_bytes.max(scratch.bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, path::Path};

    #[test]
    fn spatial_backdrop_samples_the_composed_scope_with_bounded_local_offsets() -> Result<()> {
        let gpu = Gpu::new()?;
        let dir = tempfile::tempdir()?;
        let shader = dir.path().join("neighbour.wgsl");
        fs::write(
            &shader,
            r#"
            fn backdrop_pixel(local: vec2f, size: vec2f, screen: vec2f) -> vec4f {
                if local.x < size.x * 0.5 { return backdrop_at(vec2f(200.0, 200.0)); }
                return backdrop_at(vec2f(-200.0, -200.0));
            }
        "#,
        )?;
        let size = [16, 16];
        let pixel = |x: usize, y: usize| 4 * (y * 16 + x);
        let plate: Vec<u8> = (0..16)
            .flat_map(|y| (0..16).flat_map(move |x| [x * 16, y * 16, 60, 255]))
            .collect();
        let lens = json!({"content":{"type":"backdrop","filter":{"shader":shader,"sample_radius_px":3}},
                           "mask":{"content":{"type":"rect","width":12,"height":8}}});
        let draw = |layers: serde_json::Value| -> Result<Vec<u8>> {
            let graphics: Vec<Graphic> = serde_json::from_value(layers)?;
            let (output, stats) = gpu.compose_exposed(
                super::super::FrameInputs {
                    size,
                    primary: super::super::PrimaryPicture::Video(&plate),
                    video_layers: &Default::default(),
                    scopes: vec![(&graphics, 0)],
                    overlay: None,
                },
                None,
                &mut PaintCache::default(),
            )?;
            assert_eq!(stats.backdrop_passes, 2);
            Ok(output)
        };
        // A cropped receiver retains its mask coordinates. A sample may reach
        // outside its mask and includes the earlier path in paint order.
        let ink = json!({"position":[4,8], "content":{"type":"rect","width":1,"height":1,"fill":[230,10,12,255]}});
        let cropped = draw(json!([ink, {"position":[-3,3],"content":{
            "type":"transform_group","children":[lens]}}]))?;
        assert_eq!(&cropped[pixel(1, 5)..][..4], &[230, 10, 12, 255]);
        assert_eq!(&cropped[pixel(7, 5)..][..4], &plate[pixel(4, 2)..][..4]);
        assert_eq!(&cropped[pixel(14, 5)..][..4], &plate[pixel(14, 5)..][..4]);
        // Rotate the receiver. A positive local diagonal now reads left + down.
        let rotated = draw(
            json!([{"position":[12,1],"transform":{"rotation_degrees":90},
            "content":{"type":"transform_group","children":[lens]}}]),
        )?;
        assert_eq!(&rotated[pixel(9, 3)..][..4], &plate[pixel(6, 6)..][..4]);
        Ok(())
    }

    #[test]
    fn over_budget_edit_keeps_the_previous_parked_picture() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/frost.json");
        let mut engine = crate::Engine::new(Project::load(&path)?)?;
        let before = engine.render(12)?;
        let mut next = engine.project().clone();
        next.size = [4096; 2];
        next.graphics = serde_json::from_value(json!([{
            "content": {"type":"backdrop","filter":{
                "shader":path.with_file_name("gaussian-blur.wgsl"), "sigma_px":8
            }},
            "mask":{"content":{"type":"path","d":"M0 0H4096V4096H0Z"}}
        }]))?;
        let error = engine.replace_project(next).unwrap_err();
        assert!(error.to_string().contains("256 MiB"));
        let after = engine.render(12)?;
        assert_eq!(before.trace.revision, after.trace.revision);
        assert_eq!(before.rgba, after.rgba);
        Ok(())
    }

    #[test]
    fn scratch_admission_uses_placed_mask_bounds_and_last_visible_sample() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/frost.json");
        let mut project = Project::load(&path)?;
        project.size = [4096, 4096];
        let cache = PaintCache::default();
        let mut adjustment = json!({
            "content":{"type":"backdrop","filter":{"shader":"shader.wgsl","sigma_px":16}},
            "mask":{"position":[-6, 4],"content":{"type":"path","d":"M6 -4H4102V4092H6Z"}}
        });
        let with_layer = |layer| -> Result<Project> {
            let mut edit = project.clone();
            edit.graphics = serde_json::from_value(json!([layer]))?;
            Ok(edit)
        };
        let validate =
            |layer, cache: &PaintCache| Gpu::validate_backdrop_scratch(&with_layer(layer)?, cache);
        let failure = validate(adjustment.clone(), &cache).unwrap_err();
        assert!(failure.to_string().contains("256 MiB"));
        // A translated parent clips its child's sampled/snapshot workspace.
        validate(
            json!({"position":[2048,0],"content":{"type":"group","children":[adjustment]}}),
            &cache,
        )?;
        validate(
            json!({"position":[0,-8000],"content":{"type":"group","children":[adjustment]}}),
            &cache,
        )?;
        let scaled = |last_scale: f32| {
            json!({
                "motion":[{"frame":0,"scale":0.25},{"frame":1,"scale":last_scale}],
                "content":{"type":"transform_group","children":[adjustment]}
            })
        };
        validate(scaled(0.5), &cache)?;
        assert!(validate(scaled(1.0), &cache).is_err());
        adjustment["content"]["filter"]["sigma_px"] = json!(0);
        validate(adjustment.clone(), &cache)?;
        adjustment["content"]["filter"]["sigma_px"] = json!(16);
        adjustment["wipe"] = json!([300, 600]);
        validate(adjustment.clone(), &cache)?;
        adjustment["wipe"] = json!([0, 10]);
        assert!(validate(adjustment.clone(), &cache).is_err());
        adjustment["mask"]["content"]["d"] = json!("M4 4H48V32H4Z");
        validate(adjustment.clone(), &cache)?;

        // Small endpoints hide a full-frame mask at an intermediate key.
        adjustment["mask"]["content"] = json!({"type":"rect", "height":4096,
            "width":[{"frame":0,"value":8},{"frame":2,"value":4096},{"frame":6,"value":8}]});
        assert!(validate(adjustment.clone(), &cache).is_err());
        adjustment["mask"]["content"]["height"] = json!(32);
        validate(adjustment.clone(), &cache)?;

        // External SVG metadata supplies bounds before painting. An already
        // pinned small SVG remains authoritative if its source file is replaced.
        let temp = tempfile::tempdir()?;
        let svg = temp.path().join("mask.svg");
        fs::write(&svg, r#"<svg width="16" height="16"/>"#)?;
        adjustment["mask"]["content"] = json!({"type":"svg","source":svg});
        validate(adjustment.clone(), &cache)?;
        let mut pinned = PaintCache::default();
        pinned.install_rasters(pinned.stage_rasters(&with_layer(adjustment.clone())?, None)?);
        fs::write(&svg, r#"<svg width="4096" height="4096"/>"#)?;
        assert!(validate(adjustment.clone(), &cache).is_err());
        validate(adjustment, &pinned)?;
        Ok(())
    }
}
