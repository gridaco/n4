//! Keep a pending Gaussian resolve and its following point grades in the
//! final draw. A later Gaussian needs a real texture of the preceding result.
use super::{Draw, IDENTITY_EFFECT, ScenePainter, ShaderKind};
use crate::graphics::{Blend, Content, Graphic, SampledGrade, SampledSourceBlur, SourceEffect};
use anyhow::Result;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SourceFeatures {
    pub blur: bool,
    pub grade: bool,
}

pub(super) struct PreparedSource {
    pub source: wgpu::Texture,
    pub encoded: bool,
    blur: Option<(wgpu::Texture, SampledSourceBlur)>,
    pub grades: Vec<SampledGrade>,
}

impl PreparedSource {
    pub fn new(source: wgpu::Texture, encoded: bool) -> Self {
        Self {
            source,
            encoded,
            blur: None,
            grades: Vec::new(),
        }
    }

    pub fn features(&self) -> SourceFeatures {
        SourceFeatures {
            blur: self.blur.is_some(),
            grade: !self.grades.is_empty(),
        }
    }

    pub fn blur_input(&self) -> Option<(&wgpu::Texture, SampledSourceBlur)> {
        self.blur
            .as_ref()
            .map(|(texture, params)| (texture, *params))
    }
}

enum ActiveEffect {
    Blur(SampledSourceBlur),
    Grade(SampledGrade),
}

/// Each Gaussian after an active operation materializes its input. Two
/// alternating images can serve any number of sequential materializations.
pub(super) fn stage_count(spatial: impl Iterator<Item = bool>) -> Option<usize> {
    let mut count = None;
    for (index, blur) in spatial.enumerate() {
        if blur {
            count = Some((count.unwrap_or(0) + usize::from(index > 0)).min(2));
        }
    }
    count
}

impl ScenePainter<'_> {
    pub(super) fn source_effects(
        &mut self,
        prepared: &mut PreparedSource,
        layer: &Graphic,
        content: &Content,
        frame: u32,
        local_crop: [i64; 2],
    ) -> Result<()> {
        let fraction = self.time.fraction();
        let mut active = Vec::new();
        for effect in &layer.effects {
            match effect {
                SourceEffect::RgbGrade(grade) => {
                    let sample = grade.sample(frame, fraction);
                    if sample != SampledGrade::IDENTITY {
                        active.push(ActiveEffect::Grade(sample));
                    }
                }
                SourceEffect::SourceBlur(blur) => {
                    let sample = blur.sample(frame, fraction);
                    if sample.sigma > 0. && sample.amount > 0. {
                        active.push(ActiveEffect::Blur(sample));
                    }
                }
            }
        }
        if let Some(count) = stage_count(
            active
                .iter()
                .map(|effect| matches!(effect, ActiveEffect::Blur(_))),
        ) {
            self.gpu.source_blurs.borrow_mut().reserve_stages(
                &self.gpu.device,
                prepared.source.size(),
                count,
            )?;
        }
        let mut stages = 0;
        for effect in active {
            match effect {
                ActiveEffect::Grade(grade) => prepared.grades.push(grade),
                ActiveEffect::Blur(sample) => {
                    if prepared.features() != SourceFeatures::default() {
                        self.materialize_source(prepared, local_crop, stages % 2)?;
                        stages += 1;
                    }
                    // Scratch textures are overwritten by later draws. Their
                    // handle alone cannot identify an upstream effect result.
                    let key = (stages == 0
                        && !matches!(
                            content,
                            Content::Scene3d(_) | Content::Footage { history: Some(_) }
                        ))
                    .then(|| self.source_key(&prepared.source, content));
                    let blurred =
                        self.blur_source(&prepared.source, key, prepared.encoded, sample.sigma)?;
                    prepared.blur = Some((blurred, sample));
                }
            }
        }
        Ok(())
    }

    fn materialize_source(
        &mut self,
        prepared: &mut PreparedSource,
        local_crop: [i64; 2],
        slot: usize,
    ) -> Result<()> {
        let cache = self.gpu.source_blurs.borrow();
        let stage = cache.stages[slot]
            .as_ref()
            .expect("reserved source effect surface");
        let pipeline = self.gpu.effect(
            ShaderKind::Point {
                blend: Blend::SourceOver,
                features: prepared.features(),
                intermediate: true,
            },
            IDENTITY_EFFECT,
        )?;
        self.gpu.paint(
            self.commands,
            stage,
            &prepared.source,
            Draw {
                blend: Blend::SourceOver,
                destination_art: false,
                transform: None,
                origin: local_crop.map(|v| -v),
                artwork_origin: [0; 2],
                mask: None,
                wipe_right: None,
                opacity: 1.,
                amount: 0.,
                encoded: prepared.encoded,
                blur: prepared.blur_input(),
                grades: &prepared.grades,
                object_mattes: None,
                pipeline: &pipeline,
            },
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        )?;
        self.stats.source_effect_intermediate_passes += 1;
        prepared.source = stage.clone();
        prepared.encoded = false;
        prepared.blur = None;
        prepared.grades.clear();
        Ok(())
    }
}
