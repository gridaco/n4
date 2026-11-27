//! An alpha underlay samples the finished drawable before it blends with the scene.
use super::{
    Bounds, Draw, Gpu, ScenePainter, ShaderKind, ancestor_envelope, raster_bounds,
    sampled::{
        MAX_SCRATCH_BYTES, Pass, Region, Scratch, gaussian_weights, sampling_uniform, scratch_bytes,
    },
    translated,
};
use crate::{
    Project,
    graphics::{AlphaUnderlay, Content, Graphic, PaintCache, SampleTime},
};
use anyhow::Result;

impl Bounds {
    fn expanded(self, margin: i64) -> Self {
        Self {
            left: self.left.saturating_sub(margin),
            top: self.top.saturating_sub(margin),
            right: self.right.saturating_add(margin),
            bottom: self.bottom.saturating_add(margin),
        }
    }

    fn shifted(self, [dx, dy]: [i32; 2]) -> Self {
        Self {
            left: self.left.saturating_add(i64::from(dx)),
            top: self.top.saturating_add(i64::from(dy)),
            right: self.right.saturating_add(i64::from(dx)),
            bottom: self.bottom.saturating_add(i64::from(dy)),
        }
    }

    fn region(self, relative_to: [i64; 2]) -> Region {
        Region {
            x: (self.left - relative_to[0]) as u32,
            y: (self.top - relative_to[1]) as u32,
            width: self.width(),
            height: self.height(),
        }
    }
}

struct Layout {
    // Source, horizontal rows and output in one scratch-local coordinate system.
    source: Region,
    rows: Region,
    output: Region,
    capture_origin: [i64; 2],
    destination: Region,
    radius: u32,
}

impl Layout {
    fn clipped(scope: [u32; 2], caster: Bounds, recipe: &AlphaUnderlay) -> Option<Self> {
        if caster.empty() || recipe.rgba[3] == 0 {
            return None;
        }
        let radius = (3. * recipe.sigma_px.get()).ceil() as u32;
        let r = i64::from(radius);
        // Compute in shadow destination space. A source outside the visible
        // scene can contribute to the result through either offset or blur.
        let shifted = caster.shifted(recipe.offset);
        let visible = shifted.expanded(r).intersect(Bounds::new([0; 2], scope));
        if visible.empty() {
            return None;
        }
        let needed = shifted.intersect(visible.expanded(r));
        let origin = [needed.left.min(visible.left), needed.top.min(visible.top)];
        let source = needed.region(origin);
        let output = visible.region(origin);
        let rows = Region {
            y: source.y,
            height: source.height,
            ..output
        };
        Some(Self {
            capture_origin: [
                needed.left - i64::from(recipe.offset[0]),
                needed.top - i64::from(recipe.offset[1]),
            ],
            source,
            rows,
            output,
            destination: visible.region([0; 2]),
            radius,
        })
    }

    fn surfaces(&self) -> [Region; 3] {
        if self.radius == 0 {
            // Hard shadows only read the capture; a previously warmed blur
            // workspace can still be reused.
            let pixel = Region {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            };
            [self.source, pixel, pixel]
        } else {
            [self.source, self.rows, self.output]
        }
    }
}

impl Gpu {
    pub(crate) fn validate_shadow_scratch(project: &Project, cache: &PaintCache) -> Result<()> {
        project.visit_framed_scopes(|layers, duration, size, view| {
            let [w, h] = size.map(u64::from);
            // Worst-case caster expands by the 48-pixel kernel radius on each side.
            if (w + 96) * (h + 96) * 4 + w * (h + 96) * 8 + w * h * 8 <= MAX_SCRATCH_BYTES {
                return Ok(());
            }
            validate_layers(
                ShadowScope {
                    size,
                    exposure: project.exposure,
                },
                layers,
                [0; 2],
                [0, duration],
                &mut Vec::new(),
                cache,
                view,
            )
        })
    }
}

#[derive(Clone, Copy)]
struct ShadowScope {
    size: [u32; 2],
    exposure: Option<crate::graphics::Exposure>,
}

fn validate_layers<'a>(
    scope: ShadowScope,
    layers: &'a [Graphic],
    parent: [i64; 2],
    time_span: [u32; 2],
    ancestors: &mut Vec<([i64; 2], &'a Graphic)>,
    cache: &PaintCache,
    view: Option<crate::camera::OrthoRanges>,
) -> Result<()> {
    for layer in layers {
        let origin = translated(parent, layer.placed_position());
        let [start, end] = layer.span.map_or(time_span, |[start, end]| {
            [time_span[0] + start, time_span[0] + end]
        });
        if let Content::Group { children } | Content::TransformGroup { children } = &layer.content {
            let transformed = layer.has_spatial_transform();
            if transformed {
                ancestors.push((origin, layer));
            }
            validate_layers(
                scope,
                children,
                origin,
                [start, end],
                ancestors,
                cache,
                view,
            )?;
            if transformed {
                ancestors.pop();
            }
        }
        let Some(recipe) = &layer.underlay else {
            continue;
        };
        if recipe.rgba[3] == 0 {
            continue;
        }
        if let Some(cues) = &layer.type_on {
            // Prefix shaping can change wrapping and height. Check each visible
            // held state at the widest wipe it reaches during its own interval.
            for (i, &(frame, _)) in cues.iter().enumerate() {
                if frame >= end - start {
                    break;
                }
                let until = cues
                    .get(i + 1)
                    .map_or(end - start, |&(next, _)| next.min(end - start));
                if let Some(prefix) = layer.content_at(frame, 0.) {
                    validate_caster(
                        scope,
                        layer,
                        &prefix,
                        origin,
                        [start + frame, start + until],
                        recipe,
                        ancestors,
                        cache,
                        view,
                    )?;
                }
            }
        } else {
            validate_caster(
                scope,
                layer,
                &layer.content,
                origin,
                [start, end],
                recipe,
                ancestors,
                cache,
                view,
            )?;
        }
    }
    Ok(())
}

fn validate_caster(
    scope: ShadowScope,
    layer: &Graphic,
    content: &Content,
    origin: [i64; 2],
    [start, end]: [u32; 2],
    recipe: &AlphaUnderlay,
    ancestors: &[([i64; 2], &Graphic)],
    cache: &PaintCache,
    view: Option<crate::camera::OrthoRanges>,
) -> Result<()> {
    let (art_at, size, active_last) = match content {
        Content::Footage { .. } | Content::Scene3d(_) => (origin, scope.size, end - 1),
        Content::Group { .. } => ([0; 2], scope.size, end - 1),
        Content::Video(clip) => {
            let until = (clip.start + clip.duration).min(end);
            if until <= start.max(clip.start) {
                return Ok(());
            }
            (origin, clip.size, until - 1)
        }
        content => {
            let (at, size) = if layer.text_animators.is_empty() {
                raster_bounds(content, cache)?
            } else {
                cache.text_offset_bounds(layer)?
            };
            (translated(origin, at), size, end - 1)
        }
    };
    let mut caster = Bounds::new(art_at, size);
    // An isolated group's children have already painted in destination space.
    // Its mask/wipe remain in authored coordinates. Without a mask, use the
    // entire output as a conservative bound for the rotated wipe half-plane.
    let destination_art = matches!(content, Content::Group { .. })
        && (layer.has_spatial_transform() || !ancestors.is_empty() || view.is_some());
    if let Some(mask) = &layer.mask {
        let (at, size) = raster_bounds(&mask.as_content(), cache)?;
        let mask = Bounds::new(translated(translated(origin, mask.position), at), size);
        caster = if destination_art {
            mask
        } else {
            caster.intersect(mask)
        };
    }
    if let Some(wipe) = layer.wipe {
        let last_sample = scope.exposure.map_or(0, |e| u32::from(e.samples) - 1);
        let width = SampleTime::at(active_last, scope.exposure, last_sample).reveal(wipe, size[0]);
        if width == 0 {
            return Ok(());
        }
        // The animated envelope spans multiple raster origins. A per-frame wipe
        // starts at that frame's origin, which can move across the envelope.
        let moving = !layer.text_animators.is_empty()
            || matches!(content, Content::Rect(rect) if rect.animated())
            || matches!(content, Content::Path(path) if path.animated());
        if width < size[0] && !moving && (!destination_art || layer.mask.is_some()) {
            let x = if matches!(layer.content, Content::Group { .. }) {
                origin[0]
            } else {
                art_at[0]
            };
            caster.right = caster.right.min(x + i64::from(width));
        }
    }
    let envelope = if destination_art && layer.mask.is_none() {
        caster
    } else {
        let bounds = ancestor_envelope(caster.motion_envelope(origin, layer), ancestors)
            .through_camera(scope.size, view);
        if destination_art {
            bounds.intersect(Bounds::new([0; 2], scope.size))
        } else {
            bounds
        }
    };
    if let Some(layout) = Layout::clipped(scope.size, envelope, recipe) {
        scratch_bytes(layout.surfaces())?;
    }
    Ok(())
}

impl ScenePainter<'_> {
    pub(super) fn shadow(
        &mut self,
        target: &wgpu::Texture,
        art: &wgpu::Texture,
        draw: Draw<'_>,
        recipe: &AlphaUnderlay,
    ) -> Result<()> {
        let Some(layout) =
            Layout::clipped([target.width(), target.height()], draw.bounds(art), recipe)
        else {
            return Ok(());
        };
        let mut workspace = self.gpu.sampled_scratch.borrow_mut();
        let scratch = Scratch::reserve(&mut workspace, &self.gpu.device, layout.surfaces())?;
        // Draw in the caster crop. Rebasing both texture and shader origins
        // preserves authored coordinates for the point effect and the mask.
        let [cx, cy] = layout.capture_origin;
        let local = |[x, y]: [i64; 2]| [x - cx, y - cy];
        let capture = Draw {
            origin: local(draw.origin),
            artwork_origin: local(draw.artwork_origin),
            mask: draw.mask.map(|(tex, pos)| (tex, local(pos))),
            wipe_right: draw.wipe_right.map(|x| x - cx),
            ..draw
        };
        self.gpu.paint(
            self.commands,
            &scratch.original,
            art,
            capture,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        )?;
        let alpha = if layout.radius > 0 {
            let weights = gaussian_weights(recipe.sigma_px.get(), layout.radius);
            let sampled = self
                .gpu
                .effect(ShaderKind::Sampled, self.cache.shader(&recipe.shader)?)?;
            let crop = |r: Region| Region { x: 0, y: 0, ..r };
            self.filter_pass(Pass {
                label: "horizontal shadow blur",
                inputs: &[&scratch.original],
                output: &scratch.horizontal,
                viewport: crop(layout.rows),
                pipeline: &sampled,
                uniform: &sampling_uniform(
                    layout.source,
                    layout.rows,
                    [1, 0],
                    layout.radius,
                    &weights,
                    true,
                ),
                preserve: false,
            });
            self.filter_pass(Pass {
                label: "vertical shadow blur",
                inputs: &[&scratch.horizontal],
                output: &scratch.vertical,
                viewport: crop(layout.output),
                pipeline: &sampled,
                uniform: &sampling_uniform(
                    layout.rows,
                    layout.output,
                    [0, 1],
                    layout.radius,
                    &weights,
                    true,
                ),
                preserve: false,
            });
            self.stats.shadow_passes += 2;
            self.stats.shadow_filtered_pixels += layout.rows.pixels() + layout.output.pixels();
            &scratch.vertical
        } else {
            &scratch.original
        };
        let mut tint: Vec<_> = [layout.destination.x, layout.destination.y, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        for (i, component) in recipe.rgba.into_iter().enumerate() {
            let c = f32::from(component) / 255.;
            let linear = if i == 3 {
                c
            } else if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
            tint.extend(linear.to_le_bytes());
        }
        self.filter_pass(Pass {
            label: "tint source alpha under layer",
            inputs: &[alpha],
            output: target,
            viewport: layout.destination,
            pipeline: &self.gpu.shadow_tint,
            uniform: &tint,
            preserve: true,
        });
        self.stats.shadow_passes += 2;
        self.stats.shadow_scratch_bytes = self.stats.shadow_scratch_bytes.max(scratch.bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_admission_covers_the_animated_parent_scale() -> Result<()> {
        let project = |end_scale: f32| -> Result<Project> {
            Ok(serde_json::from_value(serde_json::json!({
                "size":[4096,4096], "fps":[30,1],
                "shots":[{"background":{"color":[0,0,0]},"duration":30}],
                "graphics":[{
                    "motion":[{"frame":0,"scale":0.25},{"frame":29,"scale":end_scale}],
                    "content":{"type":"transform_group","children":[{
                        "content":{"type":"footage"},
                        "underlay":{"shader":"gaussian.wgsl","sigma_px":16,
                                   "offset":[6,8],"rgba":[0,0,0,255]}
                    }]}
                }]
            }))?)
        };
        let cache = PaintCache::default();
        Gpu::validate_shadow_scratch(&project(0.5)?, &cache)?;
        let error = Gpu::validate_shadow_scratch(&project(1.)?, &cache).unwrap_err();
        assert!(error.to_string().contains("256 MiB"));
        Ok(())
    }
}
