//! Provisional authored layers. An effect receives linear premultiplied color
//! and integer pixel coordinates local to its input. Painting and timing are separate.
pub use crate::render::affine::{MotionKey, Transform};
use crate::render::affine::{SampledPose, TransformLimits};
pub use crate::render::channels::{
    AnimatedScalar, Keyframe, PropertyChannels, Scalar, ScalarKey, Timing,
};
pub use crate::render::curve::{
    CubicContour, CurveEase, CurveKey, CurveMotion, CurvePoint, PathData,
};
pub use crate::render::path::{FillRule, NativePath, Stroke, StrokeCap, StrokeJoin, StrokeWidth};
use crate::render::path::{MeasuredPath, paint_path, paint_with_measure, validate_path};
pub use crate::render::rect::{CornerRadius, CornerValues, NativeRect, RectGeometry, RectLength};
use crate::render::svg::paint_svg;
use crate::{MAX_SURFACE_DIMENSION, render::text};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};
use unicode_segmentation::UnicodeSegmentation;
mod live_surface_cache;
mod spatial;
pub use spatial::{
    DistantLight, LiveSurface, MatteCoverage, ObjectMatte, Scene3d, SurfaceRole, TextPlane,
    UsdInstance,
};
mod source_blur;
mod source_effect;
pub(crate) use source_effect::SampledGrade;
pub use source_effect::{RgbGrade, SourceEffect};
mod text_animation;
mod text_layout;
pub(crate) use source_blur::SampledSourceBlur;
pub use source_blur::{FocusBand, SourceBlur};
pub use text_animation::{LabelText, TextAnimator, TextSpan};
use text_animation::{PreparedText, SampledRange, TextIndex};
pub(crate) use text_layout::PreparedTextLayouts;
#[cfg(feature = "host-audio")]
pub(crate) use text_layout::TextGeometry;
pub use text_layout::{
    TextAnchor, TextAnchorPoint, TextLine, TextMetrics, Underline, text_metrics,
};

/// Exposure for the authored graphics, beginning at the output picture's timestamp.
/// Decoded picture and discrete edit decisions remain held for this output frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    pub samples: u8,
    pub angle_degrees: u16,
}

impl Exposure {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (2..=8).contains(&self.samples) && (1..=360).contains(&self.angle_degrees),
            "exposure requires 2–8 samples and 1–360 degrees"
        );
        Ok(())
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct SampleTime {
    pub(crate) frame: u32,
    /// Exact fractional frame offset. All exposure samples lie within this picture.
    numerator: u32,
    denominator: u32,
}

impl SampleTime {
    pub(crate) fn at(frame: u32, exposure: Option<Exposure>, sample: u32) -> Self {
        let (numerator, denominator) = exposure.map_or((0, 1), |e| {
            (
                (2 * sample + 1) * u32::from(e.angle_degrees),
                2 * u32::from(e.samples) * 360,
            )
        });
        Self {
            frame,
            numerator,
            denominator,
        }
    }

    pub(crate) fn reveal(self, [start, end]: [u32; 2], width: u32) -> u32 {
        if self.frame < start {
            return 0;
        }
        if self.frame >= end {
            return width;
        }
        let age =
            u64::from(self.frame - start) * u64::from(self.denominator) + u64::from(self.numerator);
        (u64::from(width) * age / (u64::from(end - start) * u64::from(self.denominator))) as u32
    }

    pub(crate) fn fraction(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }

    /// Wrap in integer project frames before converting to floating point, so
    /// a short repeating animation remains seekable late in a long timeline.
    pub(crate) fn phase(self, period_frames: u32) -> f32 {
        let age = f64::from(self.frame % period_frames)
            + f64::from(self.numerator) / f64::from(self.denominator);
        ((age / f64::from(period_frames)) as f32).min(1.0f32.next_down())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Graphic {
    /// Author-facing layer name. Drawing and animation are independent of this label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: Content,
    /// Combine the finished source with the accumulated destination at its paint position.
    #[serde(default, skip_serializing_if = "Blend::is_source_over")]
    pub blend: Blend,
    #[serde(default, skip_serializing_if = "is_origin")]
    pub position: [i32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_anchor: Option<TextAnchor>,
    /// Half-open interval in the enclosing group's frames. Its start is local frame zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<[u32; 2]>,
    /// (Local frame, visible grapheme count). Null count reveals the full editable text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_on: Option<Vec<(u32, Option<u32>)>>,
    /// Ordered range effects on the fully laid-out label, on its local clock.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_animators: Vec<TextAnimator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub motion: Vec<MotionKey>,
    /// Independent scalar keys, evaluated on the same local clock as motion poses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<PropertyChannels>,
    /// Reveal from left to right between these two timeline frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wipe: Option<[u32; 2]>,
    /// Read the preceding result in order, before the terminal WGSL filter and final coverage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<SourceEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    /// Coverage in this layer's local space, applied to its finished content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    /// Sample the finished layer and use the filter's output alpha to tint an underlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay: Option<AlphaUnderlay>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    SourceOver,
    Multiply,
    DestinationOut,
}

impl Blend {
    fn is_source_over(&self) -> bool {
        *self == Self::SourceOver
    }
}

impl Graphic {
    pub(crate) fn has_motion(&self) -> bool {
        !self.motion.is_empty() || self.channels.is_some()
    }

    pub(crate) fn has_spatial_transform(&self) -> bool {
        self.transform.is_some()
            || !self.motion.is_empty()
            || self
                .channels
                .as_ref()
                .is_some_and(PropertyChannels::has_spatial)
    }

    pub(crate) fn rotation_varies(&self) -> bool {
        self.channels
            .as_ref()
            .and_then(|c| c.rotation.as_ref())
            .is_some_and(|keys| keys.windows(2).any(|pair| pair[0].value != pair[1].value))
    }

    pub(crate) fn motion_at(&self, frame: u32, fraction: f64) -> SampledPose {
        match &self.channels {
            Some(channels) => channels.sample(frame, fraction),
            None => MotionKey::sample(&self.motion, frame, fraction).into(),
        }
    }

    pub(crate) fn spatial_extrema(&self) -> impl Iterator<Item = SampledPose> + '_ {
        self.motion.iter().copied().map(SampledPose::from).chain(
            self.channels
                .iter()
                .flat_map(PropertyChannels::spatial_extrema),
        )
    }
    pub(crate) fn local_frame(&self, parent_frame: u32) -> Option<u32> {
        match self.span {
            Some([start, end]) if (start..end).contains(&parent_frame) => {
                Some(parent_frame - start)
            }
            Some(_) => None,
            None => Some(parent_frame),
        }
    }

    /// Evaluate shape geometry and paint at the exposure sample and discrete text at its
    /// containing frame. Runtime samples leave the accepted authoring data intact.
    pub(crate) fn content_at(&self, local_frame: u32, fraction: f64) -> Option<Cow<'_, Content>> {
        if let Content::Rect(rect) = &self.content
            && rect.animated()
        {
            return Some(Cow::Owned(Content::Rect(
                rect.sampled(local_frame, fraction),
            )));
        }
        if let Content::Path(path) = &self.content
            && path.animated()
        {
            return Some(Cow::Owned(Content::Path(
                path.sampled(local_frame, fraction),
            )));
        }
        let (
            Some(cues),
            Content::Label {
                text: LabelText::Plain(text),
                font,
                size,
                width,
                rgb,
                underline,
            },
        ) = (&self.type_on, &self.content)
        else {
            return Some(Cow::Borrowed(&self.content));
        };
        let passed = cues.partition_point(|(frame, _)| *frame <= local_frame);
        let count = cues[..passed].last()?.1;
        if text.is_empty() || count == Some(0) {
            return None;
        }
        let Some((end, _)) = count.and_then(|n| text.grapheme_indices(true).nth(n as usize)) else {
            return Some(Cow::Borrowed(&self.content));
        };
        Some(Cow::Owned(Content::Label {
            text: LabelText::Plain(text[..end].to_owned()),
            font: font.clone(),
            size: *size,
            width: *width,
            rgb: *rgb,
            underline: underline.clone(),
        }))
    }
}

fn is_origin(position: &[i32; 2]) -> bool {
    *position == [0, 0]
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Content {
    Scene3d(Scene3d),
    Rect(NativeRect),
    Path(NativePath),
    Image {
        source: PathBuf,
    },
    Svg {
        source: PathBuf,
    },
    Label {
        text: LabelText,
        font: PathBuf,
        size: u32,
        width: u32,
        rgb: [u8; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        underline: Option<Underline>,
    },
    /// The picture selected by the primary shot timeline.
    Footage {
        /// An authored shader samples a finite window of this shot's source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history: Option<SourceHistory>,
    },
    /// A separately timed moving picture drawn at this layer's position.
    Video(VideoClip),
    Backdrop {
        filter: BackdropFilter,
    },
    /// Children share their parent’s spatial transform and paint directly into its destination.
    TransformGroup {
        children: Vec<Graphic>,
    },
    /// Isolate children into a composited image before applying group paint.
    Group {
        children: Vec<Graphic>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VideoClip {
    pub source: PathBuf,
    /// First active frame on the project timeline.
    pub start: u32,
    /// Source in-point in project-rate ticks, resolved through the source PTS index.
    pub source_in: u32,
    pub duration: u32,
    /// Exact output width and height in layer-local pixels, before masking.
    pub size: [u32; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHistory {
    pub shader: PathBuf,
    pub frames: u32,
}

pub(crate) const MAX_SOURCE_HISTORY_FRAMES: u32 = 12;

impl VideoClip {
    pub fn source_frame(&self, frame: u32) -> Option<u32> {
        let local = frame.checked_sub(self.start)?;
        if local < self.duration {
            self.source_in.checked_add(local)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mask {
    pub content: MaskContent,
    #[serde(default, skip_serializing_if = "is_origin")]
    pub position: [i32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<CoverageShader>,
}

/// A mask-local coverage function driven by the sampled timeline time.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageShader {
    pub shader: PathBuf,
    pub period_frames: u32,
}

/// Provisional recipe for two directional passes over a sampled image.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SampledFilter {
    pub shader: PathBuf,
    pub sigma_px: FilterSigma,
}

/// Both recipes read the composition underneath this masked layer at its paint position.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum BackdropFilter {
    Separable(SampledFilter),
    Sampled2d {
        shader: PathBuf,
        sample_radius_px: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        params: Vec<ShaderParam>,
    },
}

/// Ordered, named material controls. Their values occupy xyzw of backdrop_params().
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderParam {
    pub name: String,
    pub value: AnimatedScalar<Scalar>,
}

impl BackdropFilter {
    pub fn shader(&self) -> &Path {
        match self {
            Self::Separable(filter) => &filter.shader,
            Self::Sampled2d { shader, .. } => shader,
        }
    }

    fn shader_mut(&mut self) -> &mut PathBuf {
        match self {
            Self::Separable(filter) => &mut filter.shader,
            Self::Sampled2d { shader, .. } => shader,
        }
    }
}

/// A sampled filter sees the masked, revealed RGBA source. Its output alpha
/// scales the tint behind the sharp layer. Offsets are in project pixels,
/// and the tint uses straight sRGB color and linear opacity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlphaUnderlay {
    pub shader: PathBuf,
    pub sigma_px: FilterSigma,
    pub offset: [i32; 2],
    pub rgba: [u8; 4],
}

/// Standard deviation in project pixels, bounded by the spike's sample budget.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct FilterSigma(u32);

impl FilterSigma {
    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

impl TryFrom<f32> for FilterSigma {
    type Error = anyhow::Error;

    fn try_from(sigma: f32) -> Result<Self> {
        ensure!(
            (0.0..=16.0).contains(&sigma),
            "sigma_px must be between 0 and 16"
        );
        // Canonicalize negative zero for equality and hashing.
        Ok(Self(if sigma == 0.0 { 0 } else { sigma.to_bits() }))
    }
}

impl From<FilterSigma> for f32 {
    fn from(value: FilterSigma) -> f32 {
        value.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaskContent {
    Rect(RectGeometry),
    Path {
        d: String,
        #[serde(default)]
        fill_rule: FillRule,
    },
    Svg {
        source: PathBuf,
    },
}

impl Mask {
    pub(crate) fn animated_rect(&self) -> Option<&RectGeometry> {
        match &self.content {
            MaskContent::Rect(rect) if rect.animated() => Some(rect),
            _ => None,
        }
    }

    /// Use the same painter and cache as a visible drawable. A path mask
    /// always uses filled coverage; SVG masks retain their authored alpha.
    pub(crate) fn as_content(&self) -> Content {
        match &self.content {
            MaskContent::Rect(rect) => Content::Rect(NativeRect::mask(rect.clone())),
            MaskContent::Path { d, fill_rule } => Content::Path(NativePath {
                d: d.clone().into(),
                fill_rule: *fill_rule,
                fill: Some([255; 4]),
                stroke_start: None,
                stroke_end: None,
                stroke: None,
            }),
            MaskContent::Svg { source } => Content::Svg {
                source: source.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub shader: PathBuf,
    /// Percentage passed to the shader as a normalized scalar.
    pub amount: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mattes: Vec<ObjectMatte>,
}

pub(crate) fn resolve(layers: &mut [Graphic], resolve_path: &mut dyn FnMut(&mut PathBuf)) {
    for layer in layers {
        match &mut layer.content {
            Content::Svg { source } | Content::Image { source } => resolve_path(source),
            Content::Video(clip) => resolve_path(&mut clip.source),
            Content::Footage {
                history: Some(history),
            } => resolve_path(&mut history.shader),
            Content::Scene3d(scene) => {
                for model in &mut scene.models {
                    resolve_path(&mut model.source);
                    for surface in &mut model.surfaces {
                        resolve(&mut surface.layers, resolve_path);
                    }
                }
                for plane in &mut scene.planes {
                    resolve(std::slice::from_mut(plane.text.as_mut()), resolve_path);
                }
            }
            Content::Label { font, .. } => resolve_path(font),
            Content::Backdrop { filter } => resolve_path(filter.shader_mut()),
            Content::Group { children } | Content::TransformGroup { children } => {
                resolve(children, resolve_path)
            }
            Content::Footage { history: None } | Content::Path(_) | Content::Rect(_) => {}
        }
        if let Some(Mask {
            content: MaskContent::Svg { source },
            ..
        }) = &mut layer.mask
        {
            resolve_path(source);
        }
        if let Some(filter) = &mut layer.filter {
            resolve_path(&mut filter.shader);
        }
        if let Some(underlay) = &mut layer.underlay {
            resolve_path(&mut underlay.shader);
        }
        if let Some(shader) = layer.mask.as_mut().and_then(|m| m.shader.as_mut()) {
            resolve_path(&mut shader.shader);
        }
    }
}

pub(crate) fn validate(layers: &[Graphic], timeline_end: u32) -> Result<()> {
    validate_scope(layers, timeline_end, timeline_end, false, [0; 2], None)
}

fn validate_scope(
    layers: &[Graphic],
    scope_end: u32,
    timeline_end: u32,
    isolated: bool,
    parent_origin: [i64; 2],
    parent_limits: Option<TransformLimits>,
) -> Result<()> {
    text_layout::validate_anchors(layers)?;
    for layer in layers {
        if layer.blend == Blend::DestinationOut {
            ensure!(
                isolated,
                "destination_out requires an isolated group destination"
            );
            ensure!(
                layer.underlay.is_none() && !matches!(layer.content, Content::Backdrop { .. }),
                "destination_out requires source coverage without a backdrop or underlay"
            );
        }
        let mut duration = scope_end;
        if let Some([start, end]) = layer.span {
            ensure!(
                start < end && end <= scope_end,
                "layer span must fit inside its parent"
            );
            duration = end - start;
        }
        if let Some(cues) = &layer.type_on {
            ensure!(
                matches!(
                    &layer.content,
                    Content::Label {
                        text: LabelText::Plain(_),
                        ..
                    }
                ) && layer.text_animators.is_empty(),
                "type_on requires a plain text label without text animators"
            );
            ensure!(
                !cues.is_empty() && cues.windows(2).all(|p| p[0].0 < p[1].0),
                "type_on cues require strictly increasing local frames"
            );
        }
        if let Content::Label { text, .. } = &layer.content {
            if matches!(text, LabelText::Spans(_)) || !layer.text_animators.is_empty() {
                let index = TextIndex::new(text)?;
                for animator in &layer.text_animators {
                    animator.validate(&index)?;
                }
            }
        } else {
            ensure!(
                layer.text_animators.is_empty(),
                "text_animators requires a label"
            );
        }
        if let Some(channels) = &layer.channels {
            ensure!(
                layer.motion.is_empty(),
                "use either motion poses or property channels on a layer"
            );
            channels.validate()?;
        }
        if layer.transform.is_some() || !layer.motion.is_empty() {
            if let Some(transform) = layer.transform {
                transform.validate(layer.channels.as_ref(), &layer.motion)?;
            }
            ensure!(
                layer
                    .motion
                    .windows(2)
                    .all(|pair| pair[0].frame < pair[1].frame),
                "motion keys must have increasing local frames"
            );
            for key in &layer.motion {
                key.validate()?;
            }
            ensure!(
                !matches!(&layer.content, Content::Backdrop { .. }),
                "a backdrop inherits spatial transforms from its parent"
            );
        }
        if let Some([start, end]) = layer.wipe {
            ensure!(start < end, "wipe requires start < end");
        }
        if let Some(filter) = &layer.filter {
            ensure!(filter.amount <= 100, "effect amount exceeds 100%");
            ensure!(
                filter.mattes.is_empty() || matches!(layer.content, Content::Scene3d(_)),
                "object mattes require a filter on a scene3d"
            );
        }
        for effect in &layer.effects {
            match effect {
                SourceEffect::SourceBlur(blur) => {
                    ensure!(
                        !matches!(
                            layer.content,
                            Content::Group { .. }
                                | Content::TransformGroup { .. }
                                | Content::Backdrop { .. }
                        ),
                        "source blur requires a drawable with its own source-local raster"
                    );
                    blur.validate()?;
                }
                SourceEffect::RgbGrade(grade) => {
                    ensure!(
                        !matches!(
                            layer.content,
                            Content::TransformGroup { .. } | Content::Backdrop { .. }
                        ),
                        "RGB grade requires a drawable or isolated group"
                    );
                    grade.validate()?;
                }
            }
        }
        if let Some(shader) = layer.mask.as_ref().and_then(|m| m.shader.as_ref()) {
            ensure!(
                matches!(layer.content, Content::Backdrop { .. }),
                "animated mask coverage requires a backdrop adjustment in this spike"
            );
            ensure!(
                shader.period_frames > 0,
                "coverage shader requires a positive period_frames"
            );
        }
        if let Some(mask) = &layer.mask {
            match mask.as_content() {
                Content::Path(path) => validate_path(&path).context("invalid path mask")?,
                Content::Rect(rect) => {
                    rect.raster_bounds().context("invalid rectangle mask")?;
                }
                _ => {}
            }
        }
        let origin = [
            parent_origin[0] + i64::from(layer.placed_position()[0]),
            parent_origin[1] + i64::from(layer.placed_position()[1]),
        ];
        let limits = if parent_limits.is_some()
            || layer.has_spatial_transform()
            || matches!(layer.content, Content::TransformGroup { .. })
        {
            Some(parent_limits.unwrap_or_default().child(layer, origin)?)
        } else {
            None
        };
        match &layer.content {
            Content::Footage {
                history: Some(history),
            } => ensure!(
                (1..=MAX_SOURCE_HISTORY_FRAMES).contains(&history.frames),
                "source history requires 1 to {MAX_SOURCE_HISTORY_FRAMES} prior frames"
            ),
            Content::Backdrop { filter } => {
                if let BackdropFilter::Sampled2d {
                    sample_radius_px,
                    params,
                    ..
                } = filter
                {
                    ensure!(
                        (1..=96).contains(sample_radius_px),
                        "backdrop sample radius must be between 1 and 96 output pixels"
                    );
                    ensure!(
                        params.len() <= 4,
                        "a backdrop accepts up to four scalar parameters"
                    );
                    for (i, param) in params.iter().enumerate() {
                        ensure!(
                            !param.name.trim().is_empty(),
                            "backdrop parameter needs a name"
                        );
                        ensure!(
                            !params[..i].iter().any(|p| p.name == param.name),
                            "duplicate backdrop parameter {}",
                            param.name
                        );
                        param.value.range(&param.name)?;
                    }
                }
                ensure!(
                    layer.blend == Blend::SourceOver,
                    "a backdrop adjustment replaces its destination and cannot carry a blend mode"
                );
                ensure!(
                    layer.channels.is_none(),
                    "a backdrop has no source property channels"
                );
                ensure!(layer.mask.is_some(), "backdrop adjustment requires a mask");
                ensure!(
                    layer.underlay.is_none(),
                    "an alpha underlay requires a source layer or group"
                );
                ensure!(
                    layer.filter.is_none(),
                    "use the sampled filter on the backdrop adjustment"
                );
            }
            Content::Scene3d(scene) => scene.validate(
                duration,
                timeline_end,
                layer.filter.as_ref().map_or(&[], |f| f.mattes.as_slice()),
            )?,
            Content::Path(path) => validate_path(path).context("invalid painted path")?,
            Content::Rect(rect) => {
                rect.raster_bounds().context("invalid painted rectangle")?;
            }
            Content::TransformGroup { children } => {
                ensure!(
                    layer.mask.is_none()
                        && layer.filter.is_none()
                        && layer.wipe.is_none()
                        && layer.underlay.is_none()
                        && layer.blend == Blend::SourceOver
                        && layer.channels.as_ref().is_none_or(|c| c.opacity.is_none())
                        && layer.motion.iter().all(|key| key.opacity == 1.),
                    "transform_group accepts spatial motion only; apply paint and opacity to a drawable"
                );
                validate_scope(children, duration, timeline_end, isolated, origin, limits)?;
            }
            Content::Group { children } => {
                validate_scope(children, duration, timeline_end, true, origin, limits)?
            }
            Content::Video(clip) => {
                ensure!(clip.duration > 0, "video layer duration must be positive");
                ensure!(
                    clip.start
                        .checked_add(clip.duration)
                        .is_some_and(|end| end <= timeline_end),
                    "video layer extends beyond the project"
                );
                clip.source_in
                    .checked_add(clip.duration)
                    .context("video layer source range overflow")?;
                ensure!(
                    clip.size
                        .iter()
                        .all(|n| (1..=MAX_SURFACE_DIMENSION).contains(n)),
                    "invalid video layer dimensions"
                );
            }
            Content::Label {
                width,
                size,
                underline,
                ..
            } => {
                if let Some(underline) = underline {
                    underline.validate()?;
                }
                ensure!(
                    (1..=MAX_SURFACE_DIMENSION).contains(width) && (1..=512).contains(size),
                    "invalid label dimensions"
                );
                text_animation::offset_bounds(&layer.text_animators, [*width, 1])?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn uses_footage(layers: &[Graphic]) -> bool {
    layers.iter().any(|layer| match &layer.content {
        Content::Footage { .. } => true,
        Content::Group { children } | Content::TransformGroup { children } => {
            uses_footage(children)
        }
        _ => false,
    })
}

/// With no clock this includes every placement. A sampled clock visits only active layers.
pub(crate) fn history_depth(layers: &[Graphic], clock: Option<u32>) -> u32 {
    layers
        .iter()
        .filter_map(|layer| {
            let local = match clock {
                Some(frame) => Some(layer.local_frame(frame)?),
                None => None,
            };
            match &layer.content {
                Content::Footage {
                    history: Some(history),
                } => Some(history.frames),
                Content::Group { children } | Content::TransformGroup { children } => {
                    Some(history_depth(children, local))
                }
                _ => None,
            }
        })
        .max()
        .unwrap_or(0)
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct PaintStats {
    pub history_uploads: u32,
    pub history_passes: u32,
    pub raster_hits: u32,
    pub raster_paints: u32,
    /// Animated paragraphs newly shaped while rendering this frame.
    pub text_layouts: u32,
    pub filtered_layers: u32,
    pub graphic_uploads: u32,
    pub spatial_passes: u32,
    /// Object coverage redraws resolved into named filter inputs, over all exposure samples.
    pub object_matte_passes: u32,
    /// Peak logical allocation for the owner, multisample coverage and resolved mask array.
    pub object_matte_scratch_bytes: u64,
    pub mesh_uploads: u32,
    pub temporal_samples: u32,
    /// Destination pixels covered by Multiply draws, summed across exposure samples.
    pub multiply_pixels: u64,
    /// Bytes copied GPU-to-GPU to read the current destination for Multiply.
    pub multiply_copy_bytes: u64,
    /// Horizontal and vertical convolutions, summed over exposure samples.
    pub source_blur_passes: u32,
    /// Source-local resolves required between authored effects before a later blur.
    pub source_effect_intermediate_passes: u32,
    pub source_blur_filtered_pixels: u64,
    /// Reuses of the blurred source, independent of focus-band or layer edits.
    pub source_blur_cache_hits: u32,
    /// Peak bytes of a convolution issued this frame, including its output and ordered source stages.
    pub source_blur_scratch_bytes: u64,
    /// Horizontal, vertical and masked replacement draws issued for this frame.
    pub backdrop_passes: u32,
    /// Shaded texels in the two directional filter draws.
    pub backdrop_filtered_pixels: u64,
    /// Bytes copied GPU-to-GPU for the unfiltered input to the replacement draw.
    pub backdrop_copy_bytes: u64,
    /// Largest logical workspace used by an active adjustment in this frame.
    /// The GPU can retain that allocation between frames, including bypasses.
    pub backdrop_scratch_bytes: u64,
    /// GPU passes issued by source-alpha underlays, summed over exposure samples.
    pub shadow_passes: u32,
    /// Texels shaded in the underlay's directional filter passes.
    pub shadow_filtered_pixels: u64,
    /// Largest logical underlay workspace used by this frame.
    pub shadow_scratch_bytes: u64,
}

const TRANSIENT_TEXTURE_BYTES: usize = 32 << 20;
const TRANSIENT_TEXTURE_ENTRIES: usize = 64;

#[derive(Default)]
pub(crate) struct PaintCache {
    pub(crate) live_surfaces: live_surface_cache::LiveSurfaceCache,
    pub(crate) meshes: HashMap<(PathBuf, String), Arc<nothing_spatial::Mesh>>,
    pub(crate) mesh_buffers: HashMap<(PathBuf, String), nothing_spatial_wgpu::MeshBuffers>,
    rasters: HashMap<Content, Arc<Raster>>,
    pub(crate) textures: HashMap<Content, (wgpu::Texture, [i32; 2])>,
    faces: HashMap<PathBuf, skia_safe::Typeface>,
    text: HashMap<Content, Arc<PreparedText>>,
    layouts: HashMap<TextLayoutKey, Weak<text::ShapedLabel>>,
    // Key by authored centerline, so retiming either endpoint retains its measurement.
    measured_paths: HashMap<PathData, MeasuredPath>,
    // Partial text and evaluated contours share a bounded GPU-only cache.
    samples: VecDeque<CachedSample>,
    sample_bytes: usize,
    shaders: HashMap<PathBuf, String>,
}

/// CPU artwork prepared from the observed inputs of one accepted revision.
/// GPU textures remain private to each renderer.
#[derive(Default)]
pub(crate) struct PreparedSvgAssets {
    rasters: HashMap<PathBuf, Arc<Raster>>,
}

impl PreparedSvgAssets {
    #[cfg(feature = "host-audio")]
    pub(crate) fn prepare(
        project: &crate::Project,
        previous: Option<(&Self, &crate::project_watch::ProjectInputs)>,
        inputs: &crate::project_watch::ProjectInputs,
    ) -> Result<Self> {
        let mut assets = Self::default();
        for content in sources(project.graphic_scopes()).rasters {
            let Content::Svg { source } = content else {
                continue;
            };
            let reuse = previous
                .filter(|(_, observations)| inputs.same_file(observations, &source))
                .and_then(|(assets, _)| assets.rasters.get(&source));
            let raster = match reuse {
                Some(raster) => Arc::clone(raster),
                None => Arc::new(paint_svg(&source)?),
            };
            assets.rasters.insert(source, raster);
        }
        Ok(assets)
    }

    #[cfg(feature = "host-audio")]
    pub(crate) fn get(&self, path: &Path) -> Option<&Raster> {
        self.rasters.get(path).map(Arc::as_ref)
    }
}

/// Decoded dimensions of image sources observed by the accepted revision.
/// Selection can resolve an image's rectangle without loading its pixels.
#[cfg(feature = "host-audio")]
#[derive(Default)]
pub(crate) struct PreparedImageAssets {
    sizes: HashMap<PathBuf, [u32; 2]>,
}

#[cfg(feature = "host-audio")]
impl PreparedImageAssets {
    pub(crate) fn prepare(
        project: &crate::Project,
        previous: Option<(&Self, &crate::project_watch::ProjectInputs)>,
        inputs: &crate::project_watch::ProjectInputs,
    ) -> Result<Self> {
        let mut assets = Self::default();
        for content in sources(project.graphic_scopes()).rasters {
            let Content::Image { source } = content else {
                continue;
            };
            let size = previous
                .filter(|(_, observations)| inputs.same_file(observations, &source))
                .and_then(|(assets, _)| assets.get(&source));
            let size = match size {
                Some(size) => size,
                None => crate::render::image::dimensions(&source)?,
            };
            assets.sizes.insert(source, size);
        }
        Ok(assets)
    }

    pub(crate) fn get(&self, path: &Path) -> Option<[u32; 2]> {
        self.sizes.get(path).copied()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextLayoutKey {
    copy: Arc<str>,
    font: PathBuf,
    size: u32,
    width: u32,
}

struct CachedSample {
    content: Content,
    ranges: Option<Vec<SampledRange>>,
    texture: wgpu::Texture,
    origin: [i32; 2],
    bytes: usize,
}

#[derive(Default)]
struct Sources<'a> {
    instances: Vec<&'a UsdInstance>,
    meshes: HashSet<(PathBuf, String)>,
    rasters: HashSet<Content>,
    animated_text: HashMap<Content, Vec<&'a [TextAnimator]>>,
    shaders: HashSet<&'a Path>,
    measured_paths: HashSet<&'a PathData>,
}

fn sources<'a>(scopes: impl IntoIterator<Item = &'a [Graphic]>) -> Sources<'a> {
    fn visit<'a>(layers: &'a [Graphic], sources: &mut Sources<'a>) {
        for layer in layers {
            if let Some(effect) = &layer.filter {
                sources.shaders.insert(&effect.shader);
            }
            if let Some(underlay) = &layer.underlay {
                sources.shaders.insert(&underlay.shader);
            }
            if let Some(mask) = &layer.mask {
                if mask.animated_rect().is_none() {
                    sources.rasters.insert(mask.as_content());
                }
                if let Some(shader) = &mask.shader {
                    sources.shaders.insert(&shader.shader);
                }
            }
            match &layer.content {
                Content::Footage {
                    history: Some(history),
                } => {
                    sources.shaders.insert(&history.shader);
                }
                Content::Backdrop { filter } => {
                    sources.shaders.insert(filter.shader());
                }
                Content::Group { children } | Content::TransformGroup { children } => {
                    visit(children, sources)
                }
                Content::Rect(rect) if rect.animated() => {}
                Content::Path(path) if path.animated() => {
                    if path.trim_animated() && !matches!(path.d, PathData::Animated(_)) {
                        sources.measured_paths.insert(&path.d);
                    }
                }
                Content::Scene3d(scene) => {
                    for model in &scene.models {
                        sources.instances.push(model);
                        sources
                            .meshes
                            .insert((model.source.clone(), model.prim.clone()));
                        for surface in &model.surfaces {
                            visit(&surface.layers, sources);
                        }
                    }
                    for plane in &scene.planes {
                        visit(std::slice::from_ref(plane.text.as_ref()), sources);
                    }
                }
                Content::Footage { history: None } | Content::Video(_) => {}
                content if !layer.text_animators.is_empty() => {
                    sources
                        .animated_text
                        .entry(content.clone())
                        .or_default()
                        .push(&layer.text_animators);
                }
                content => {
                    sources.rasters.insert(content.clone());
                }
            }
        }
    }
    let mut sources = Sources::default();
    for layers in scopes {
        visit(layers, &mut sources);
    }
    sources
}

pub(crate) fn validate_document_artwork(project: &crate::Project) -> Result<()> {
    let cache = PaintCache::default();
    let mut project = project.clone();
    let mut measured = PaintCache::default();
    measured.place_text_anchors(&mut project, None)?;
    cache.stage_with_layout(&project, None, measured)?;
    for shader in sources(project.graphic_scopes()).shaders {
        fs::read_to_string(shader).with_context(|| format!("read shader {}", shader.display()))?;
    }
    Ok(())
}

impl PaintCache {
    pub(crate) fn install_svg_assets(&mut self, assets: &PreparedSvgAssets) {
        self.rasters
            .extend(assets.rasters.iter().map(|(path, raster)| {
                (
                    Content::Svg {
                        source: path.clone(),
                    },
                    Arc::clone(raster),
                )
            }));
    }

    /// Probe a transient prefix during workspace validation using the same face as playback.
    pub(crate) fn probe_label_bounds(&self, content: &Content) -> Result<([i32; 2], [u32; 2])> {
        let raster = paint_label(content, &mut self.faces.clone())?;
        Ok((raster.origin, raster.size))
    }

    pub(crate) fn text_offset_bounds(&self, layer: &Graphic) -> Result<([i32; 2], [u32; 2])> {
        let (origin, size) = match (&layer.content, self.text.get(&layer.content)) {
            (Content::Label { underline, .. }, Some(text)) => {
                text.shape.decoration_bounds(underline.as_ref())
            }
            _ => self.probe_label_bounds(&layer.content)?,
        };
        let (offset, size) = text_animation::offset_bounds(&layer.text_animators, size)?;
        Ok(([origin[0] + offset[0], origin[1] + offset[1]], size))
    }

    pub(crate) fn cached_raster_bounds(&self, content: &Content) -> Option<([i32; 2], [u32; 2])> {
        self.rasters
            .get(content)
            .map(|raster| (raster.origin, raster.size))
    }

    pub(crate) fn prepare_spatial_meshes(&mut self, project: &crate::Project) -> Result<()> {
        let spatial_scopes = project
            .graphic_scopes()
            .filter(|layers| crate::camera::contains_scene3d(layers));
        let inputs = sources(spatial_scopes);
        for key in inputs.meshes {
            self.import_mesh(key)?;
        }
        self.validate_model_bindings(&inputs.instances, None)
    }

    fn validate_model_bindings(
        &self,
        models: &[&UsdInstance],
        accepted: Option<&Self>,
    ) -> Result<()> {
        for model in models
            .iter()
            .filter(|m| !m.parts.is_empty() || !m.surfaces.is_empty())
        {
            let key = (model.source.clone(), model.prim.clone());
            let mesh = self
                .meshes
                .get(&key)
                .or_else(|| accepted.and_then(|old| old.meshes.get(&key)))
                .context("missing prepared mesh for spatial binding")?;
            for part in &model.parts {
                ensure!(
                    mesh.parts.iter().any(|p| p.path == part.prim),
                    "USD {} at {} has no part {}; update the part path",
                    model.source.display(),
                    model.prim,
                    part.prim
                );
            }
            for surface in &model.surfaces {
                let slot = mesh
                    .materials
                    .iter()
                    .position(|m| m.slot == surface.slot)
                    .with_context(|| {
                        format!(
                            "USD {} at {} has no material slot {}; update the surface binding",
                            model.source.display(),
                            model.prim,
                            surface.slot
                        )
                    })?;
                ensure!(
                    mesh.indices.as_chunks::<3>().0.iter().any(|triangle| {
                        let a = mesh.vertices[triangle[0] as usize];
                        if a.material as usize != slot {
                            return false;
                        }
                        let b = mesh.vertices[triangle[1] as usize].uv;
                        let c = mesh.vertices[triangle[2] as usize].uv;
                        (b[0] - a.uv[0]) * (c[1] - a.uv[1]) != (c[0] - a.uv[0]) * (b[1] - a.uv[1])
                    }),
                    "USD {} at {} material slot {} has no usable UV area; author st coordinates on the bound faces",
                    model.source.display(),
                    model.prim,
                    surface.slot
                );
            }
        }
        Ok(())
    }

    fn import_mesh(&mut self, key: (PathBuf, String)) -> Result<()> {
        let mesh = nothing_openusd::load(&key.0, &key.1)?;
        self.meshes.insert(key, Arc::new(mesh));
        Ok(())
    }

    /// An edit stages new raster sources before committing. Asset reads and
    /// painting can fail, and the accepted revision keeps its own cache.
    pub(crate) fn stage_rasters(
        &self,
        project: &crate::Project,
        previous: Option<&crate::Project>,
    ) -> Result<Self> {
        self.stage_with_layout(project, previous, Self::default())
    }

    pub(crate) fn stage_with_layout(
        &self,
        project: &crate::Project,
        previous: Option<&crate::Project>,
        mut staged: Self,
    ) -> Result<Self> {
        let needed = sources(project.graphic_scopes());
        let accepted = previous
            .map(|p| sources(p.graphic_scopes()))
            .unwrap_or_default();
        for key in needed.meshes.difference(&accepted.meshes) {
            if !self.meshes.contains_key(key) {
                staged.import_mesh(key.clone())?;
            }
        }
        staged.validate_model_bindings(&needed.instances, Some(self))?;
        for d in &needed.measured_paths {
            if !self.measured_paths.contains_key(*d) {
                staged
                    .measured_paths
                    .insert((*d).clone(), MeasuredPath::new(d)?);
            }
        }
        for (content, effects) in &needed.animated_text {
            let label = if let Some(label) = self.text.get(content) {
                label
            } else {
                staged.prepare_text(content, Some(self), &mut PaintStats::default())?;
                &staged.text[content]
            };
            for animators in effects {
                let Content::Label { underline, .. } = content else {
                    unreachable!()
                };
                text_animation::offset_bounds(
                    animators,
                    label.shape.decoration_bounds(underline.as_ref()).1,
                )?;
            }
        }
        for content in needed.rasters.difference(&accepted.rasters) {
            // Accepted SVG artwork was seeded before validating the renderer.
            if matches!(content, Content::Svg { .. }) && self.rasters.contains_key(content) {
                continue;
            }
            if self.text.contains_key(content)
                || matches!(
                    content,
                    Content::Label {
                        underline: Some(_),
                        ..
                    }
                )
            {
                staged.prepare_text(content, Some(self), &mut PaintStats::default())?;
            }
            if let Content::Label { font, .. } = content
                && let Some(face) = self.faces.get(font)
            {
                staged.faces.insert(font.clone(), face.clone());
            }
            staged.raster(content, &mut PaintStats::default())?;
        }
        Ok(staged)
    }

    pub(crate) fn install_rasters(&mut self, staged: Self) {
        self.meshes.extend(staged.meshes);
        self.rasters.extend(staged.rasters);
        self.text.extend(staged.text);
        self.layouts.extend(staged.layouts);
        self.faces.extend(staged.faces);
        self.measured_paths.extend(staged.measured_paths);
    }

    pub(crate) fn retain(&mut self, project: &crate::Project) {
        let needed = sources(project.graphic_scopes());
        self.live_surfaces.retain(|surface| {
            needed
                .instances
                .iter()
                .any(|m| m.surfaces.contains(surface))
        });
        self.meshes.retain(|k, _| needed.meshes.contains(k));
        self.mesh_buffers.retain(|k, _| needed.meshes.contains(k));
        self.measured_paths
            .retain(|d, _| needed.measured_paths.contains(d));
        self.rasters
            .retain(|source, _| needed.rasters.contains(source));
        self.textures
            .retain(|source, _| needed.rasters.contains(source));
        self.text.retain(|source, _| {
            needed.animated_text.contains_key(source) || needed.rasters.contains(source)
        });
        self.layouts.retain(|_, shape| shape.strong_count() > 0);
        self.shaders
            .retain(|path, _| needed.shaders.contains(path.as_path()));
        let fonts: HashSet<_> = needed
            .rasters
            .iter()
            .chain(needed.animated_text.keys())
            .filter_map(|content| match content {
                Content::Label { font, .. } => Some(font),
                _ => None,
            })
            .collect();
        self.faces.retain(|font, _| fonts.contains(font));
        self.samples.retain(|sample| match &sample.content {
            content if sample.ranges.is_some() => needed.animated_text.contains_key(content),
            Content::Label { font, .. } => fonts.contains(font),
            _ => true,
        });
        self.sample_bytes = self.samples.iter().map(|entry| entry.bytes).sum();
    }

    pub(crate) fn shader(&mut self, path: &Path) -> Result<&str> {
        if !self.shaders.contains_key(path) {
            self.shaders.insert(
                path.to_owned(),
                fs::read_to_string(path)
                    .with_context(|| format!("read effect {}", path.display()))?,
            );
        }
        Ok(&self.shaders[path])
    }

    pub(crate) fn raster(&mut self, content: &Content, stats: &mut PaintStats) -> Result<&Raster> {
        if matches!(
            content,
            Content::Label {
                underline: Some(_),
                ..
            }
        ) {
            self.prepare_text(content, None, stats)?;
        }
        match self.rasters.entry(content.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                stats.raster_hits += 1;
                Ok(&**entry.into_mut())
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let raster = match content {
                    Content::Rect(rect) => rect.paint()?,
                    Content::Path(path) => paint_path(path)?,
                    Content::Svg { source } => paint_svg(source)?,
                    Content::Image { source } => crate::render::image::paint_image(source)?,
                    Content::Label { rgb, underline, .. } if self.text.contains_key(content) => {
                        self.text[content].paint(*rgb, &[], underline.as_ref())?
                    }
                    Content::Label { .. } => paint_label(content, &mut self.faces)?,
                    _ => bail!("requested a raster for a composited input"),
                };
                stats.raster_paints += 1;
                Ok(&**entry.insert(Arc::new(raster)))
            }
        }
    }

    fn prepare_text(
        &mut self,
        content: &Content,
        accepted: Option<&Self>,
        stats: &mut PaintStats,
    ) -> Result<()> {
        if self.text.contains_key(content) {
            return Ok(());
        }
        if let Some(label) = accepted.and_then(|cache| cache.text.get(content)) {
            self.text.insert(content.clone(), Arc::clone(label));
            return Ok(());
        }
        let Content::Label {
            text,
            font,
            size,
            width,
            ..
        } = content
        else {
            bail!("text animator requires a label")
        };
        let index = TextIndex::new(text)?;
        let key = TextLayoutKey {
            copy: index.source.clone(),
            font: font.clone(),
            size: *size,
            width: *width,
        };
        let reusable = self
            .layouts
            .get(&key)
            .or_else(|| accepted.and_then(|cache| cache.layouts.get(&key)))
            .and_then(Weak::upgrade);
        let shape = if let Some(shape) = reusable {
            shape
        } else {
            let face = match self.faces.entry(font.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let face = match accepted.and_then(|c| c.faces.get(font)) {
                        Some(face) => face.clone(),
                        None => text::load_face(font)?,
                    };
                    entry.insert(face)
                }
            };
            stats.text_layouts += 1;
            Arc::new(text::ShapedLabel::new(
                &index.source,
                font,
                face.clone(),
                *size,
                *width,
            )?)
        };
        self.layouts.insert(key, Arc::downgrade(&shape));
        self.text
            .insert(content.clone(), Arc::new(PreparedText { index, shape }));
        Ok(())
    }

    pub(crate) fn sample_texture(
        &mut self,
        content: &Content,
        animation: Option<(&[TextAnimator], u32, f64)>,
        stats: &mut PaintStats,
        upload: impl FnOnce(&Raster) -> Result<wgpu::Texture>,
    ) -> Result<(wgpu::Texture, [i32; 2])> {
        let ranges = if let Some((animators, frame, fraction)) = animation {
            self.prepare_text(content, None, stats)?;
            Some(
                self.text[content]
                    .index
                    .sample(animators, frame, fraction)?,
            )
        } else {
            None
        };
        if let Some(index) = self
            .samples
            .iter()
            .rposition(|sample| &sample.content == content && sample.ranges == ranges)
        {
            let entry = self.samples.remove(index).expect("located cached sample");
            let picture = (entry.texture.clone(), entry.origin);
            self.samples.push_back(entry);
            stats.raster_hits += 1;
            return Ok(picture);
        }
        let raster = match (content, &ranges) {
            (Content::Label { rgb, underline, .. }, Some(ranges)) => {
                self.text[content].paint(*rgb, ranges, underline.as_ref())?
            }
            (Content::Rect(rect), _) => rect.paint()?,
            (Content::Path(path), _) => paint_with_measure(path, self.measured_paths.get(&path.d))?,
            _ => paint_label(content, &mut self.faces)?,
        };
        let bytes = raster.rgba.len();
        let texture = upload(&raster)?;
        stats.raster_paints += 1;
        stats.graphic_uploads += 1;
        if bytes <= TRANSIENT_TEXTURE_BYTES {
            while self.sample_bytes + bytes > TRANSIENT_TEXTURE_BYTES
                || self.samples.len() >= TRANSIENT_TEXTURE_ENTRIES
            {
                let evicted = self.samples.pop_front().expect("occupied sample cache");
                self.sample_bytes -= evicted.bytes;
            }
            self.samples.push_back(CachedSample {
                content: content.clone(),
                ranges,
                texture: texture.clone(),
                origin: raster.origin,
                bytes,
            });
            self.sample_bytes += bytes;
        }
        Ok((texture, raster.origin))
    }
}

fn paint_label(
    content: &Content,
    faces: &mut HashMap<PathBuf, skia_safe::Typeface>,
) -> Result<Raster> {
    let Content::Label {
        text: value,
        font,
        size,
        width,
        rgb,
        underline,
    } = content
    else {
        bail!("expected a text label");
    };
    let face = match faces.entry(font.clone()) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(text::load_face(font)?),
    };
    if underline.is_some() {
        let label = text::ShapedLabel::new(&value.full_text(), font, face.clone(), *size, *width)?;
        return label.paint_decorated(
            |_| text::GlyphStyle {
                rgba: [rgb[0], rgb[1], rgb[2], 255],
                offset: [0.; 2],
            },
            underline.as_ref(),
        );
    }
    let mut p = text::paint_with_face(&value.full_text(), font, face.clone(), *size, *width, None)?;
    for pixel in p.rgba.as_chunks_mut::<4>().0 {
        for channel in 0..3 {
            pixel[channel] = ((u16::from(rgb[channel]) * u16::from(pixel[3]) + 127) / 255) as u8;
        }
    }
    Ok(Raster {
        origin: [0, 0],
        size: [p.width, p.height],
        rgba: p.rgba,
    })
}

pub(crate) struct Raster {
    /// Position of texel (0,0) in the drawable's authored local space.
    pub origin: [i32; 2],
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Project};

    #[test]
    fn backdrop_sampling_contract_has_one_bounded_footprint() -> Result<()> {
        let shader = "optics.wgsl";
        let make = |radius: u32| -> Result<Graphic> {
            Ok(serde_json::from_value(serde_json::json!({
                "content":{"type":"backdrop","filter":{"shader":shader,"sample_radius_px":radius}},
                "mask":{"content":{"type":"rect","width":12,"height":8}}
            }))?)
        };
        for radius in [1, 96] {
            validate(&[make(radius)?], 1)?;
        }
        for radius in [0, 97] {
            assert!(validate(&[make(radius)?], 1).is_err());
        }
        for invalid in [
            serde_json::json!({"shader":shader,"sample_radius_px":4,"sigma_px":1}),
            serde_json::json!({"shader":shader,"sample_radius_px":4,"typo":true}),
            serde_json::json!({"shader":shader}),
        ] {
            assert!(serde_json::from_value::<BackdropFilter>(invalid).is_err());
        }
        Ok(())
    }

    #[test]
    fn backdrop_material_parameter_slots_are_unambiguous() -> Result<()> {
        use serde_json::json;
        let make = |params| -> Result<Graphic> {
            Ok(serde_json::from_value(json!({
                "content":{"type":"backdrop","filter":{"shader":"material.wgsl","sample_radius_px":8,"params":params}},
                "mask":{"content":{"type":"rect","width":12,"height":8}}
            }))?)
        };
        let slot = |name| json!({"name":name,"value":1});
        validate(
            &[make(json!([slot("a"), slot("b"), slot("c"), slot("d")]))?],
            8,
        )?;
        for params in [
            json!([slot(" "), slot("b")]),
            json!([slot("a"), slot("a")]),
            json!([slot("a"), slot("b"), slot("c"), slot("d"), slot("e")]),
            json!([{"name":"a","value":[{"frame":4,"value":1},{"frame":2,"value":3}]}]),
        ] {
            assert!(validate(&[make(params)?], 8).is_err());
        }
        Ok(())
    }

    #[test]
    fn nested_transform_range_is_checked_before_rendering() -> Result<()> {
        let hierarchy = |scale: f64| -> Result<Graphic> {
            Ok(serde_json::from_value(serde_json::json!({
                "motion":[{"scale":scale}],"content":{"type":"transform_group","children":[{
                    "motion":[{"scale":scale}],"content":{"type":"footage"}
                }]}
            }))?)
        };
        validate(&[hierarchy(4.)?], 3)?;
        for scale in [1e22, 1e-25] {
            let error = validate(&[hierarchy(scale)?], 3).unwrap_err();
            assert!(error.to_string().contains("GPU coordinate range"));
        }
        Ok(())
    }

    #[test]
    fn local_prefix_holds_and_seeks_across_complete_graphemes() -> Result<()> {
        let group: Graphic = serde_json::from_value(serde_json::json!({
            "span": [10,22],
            "content": {"type": "group", "children": [{
                "span": [2,10],
                "content": {"type":"label","text":"a\u{301}👩‍🔬B","font":"font.ttf",
                            "size":20,"width":100,"rgb":[0,0,0]},
                "type_on":[[1,1],[4,2],[6,null]]
            }]}
        }))?;
        validate(std::slice::from_ref(&group), 30)?;
        let Content::Group { children } = &group.content else {
            unreachable!()
        };
        let child = &children[0];
        let visible = |frame| -> Option<String> {
            let local = child.local_frame(group.local_frame(frame)?)?;
            let sampled = child.content_at(local, 0.)?;
            let Content::Label { text, .. } = sampled.as_ref() else {
                unreachable!()
            };
            Some(text.full_text().into_owned())
        };
        assert_eq!(visible(18).as_deref(), Some("a\u{301}👩‍🔬B"));
        assert_eq!(visible(12), None);
        assert_eq!(visible(16).as_deref(), Some("a\u{301}👩‍🔬"));
        assert_eq!(visible(20), None);
        assert_eq!(visible(15).as_deref(), Some("a\u{301}"));
        assert_eq!(visible(13), visible(15));
        assert_eq!(visible(22), None);
        Ok(())
    }

    #[test]
    fn rejects_invalid_timing_before_rendering() -> Result<()> {
        let mut label: Graphic = serde_json::from_value(serde_json::json!({
            "content":{"type":"label","text":"ab","font":"font.ttf","size":12,"width":40,"rgb":[0,0,0]},
            "span":[0,10], "type_on":[[4,2],[4,1]]
        }))?;
        assert!(validate(std::slice::from_ref(&label), 10).is_err());
        label.type_on = Some(vec![(4, Some(9))]);
        validate(std::slice::from_ref(&label), 10)?;
        assert!(matches!(label.content_at(4, 0.), Some(Cow::Borrowed(_))));
        label.content = Content::Footage { history: None };
        assert!(validate(std::slice::from_ref(&label), 10).is_err());
        label.type_on = None;
        label.span = Some([2, 2]);
        assert!(validate(std::slice::from_ref(&label), 10).is_err());
        label.span = Some([5, 11]);
        assert!(validate(std::slice::from_ref(&label), 10).is_err());
        Ok(())
    }

    #[test]
    fn a_motion_curve_reuses_held_pictures_and_evicts_old_samples() -> Result<()> {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/title.json");
        let mut project = Project::load(&fixture)?;
        project.shots.truncate(1);
        project.shots[0].source_in = 0;
        project.shots[0].duration = 90;
        project.title.start = 90;
        let p = serde_json::json!({"type":"path","fill":null,
            "stroke":{"rgba":[255,220,24,255],"width":4},
            "d":{"keys":[{"frame":0,"points":[[0,20],[15,20],[45,20],[60,20]]},
                         {"frame":88,"points":[[0,20],[15,64],[45,64],[60,20]]}]}});
        project.graphics = serde_json::from_value(serde_json::json!([{
            "span":[1,90],"content":{"type":"group","children":[{"content":p,"position":[10,3]}]}
        }]))?;
        let mut engine = Engine::new(project.clone())?;
        let first = engine.render(1)?;
        assert_eq!(first.trace.graphics.raster_paints, 1);
        assert_eq!(engine.render(1)?.trace.graphics.raster_paints, 0);
        for frame in 2..=89 {
            assert_eq!(engine.render(frame)?.trace.graphics.raster_paints, 1);
        }
        assert!(engine.graphics.samples.len() <= 64 && engine.graphics.sample_bytes <= 32 << 20);
        assert!(engine.graphics.rasters.is_empty() && engine.graphics.textures.is_empty());
        let revisit = engine.render(1)?;
        assert_eq!(revisit.trace.graphics.raster_paints, 1);
        assert_eq!(revisit.rgba, first.rgba);

        // Project frame 45 samples local frame 44, exactly half the interval.
        // A separately authored SVG path has the same controls and raster origin.
        let mut baked = project;
        let Content::Group { children } = &mut baked.graphics[0].content else {
            unreachable!()
        };
        let Content::Path(path) = &mut children[0].content else {
            unreachable!()
        };
        path.d = "M0 20 C15 42 45 42 60 20".into();
        assert_eq!(
            Engine::new(baked)?.render(45)?.rgba,
            engine.render(45)?.rgba
        );
        Ok(())
    }

    #[test]
    fn accepted_label_font_remains_available_for_previously_unseen_prefixes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let font = dir.path().join("project.ttf");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf"),
            &font,
        )?;
        let label: Graphic = serde_json::from_value(serde_json::json!({
            "content":{"type":"label","text":"Going further","font":font,
                       "size":28,"width":100,"rgb":[28,50,72]},
            "type_on":[[0,5],[12,null]]
        }))?;
        let mut cache = PaintCache::default();
        let mut project: Project = serde_json::from_value(serde_json::json!({
            "size":[128,64], "fps":[30,1],
            "shots":[{"background":{"color":[0,0,0]},"duration":15}]
        }))?;
        project.shots[0].graphics.push(label.clone());
        cache.install_rasters(cache.stage_rasters(&project, None)?);
        // A live session pins the face until the user explicitly refreshes assets.
        fs::remove_file(font)?;
        let sampled = label.content_at(0, 0.).expect("visible prefix");
        let prefix = paint_label(sampled.as_ref(), &mut cache.faces)?;
        assert!(
            prefix
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|rgba| rgba[3] != 0)
        );
        assert_eq!(cache.rasters.len(), 1);
        Ok(())
    }

    #[test]
    fn animated_coverage_uses_timeline_phase_on_long_timelines_and_subframes() {
        let shutter = Some(Exposure {
            samples: 2,
            angle_degrees: 360,
        });
        assert_eq!(SampleTime::at(4_000_000_000, shutter, 1).phase(4), 0.1875);
        assert!(SampleTime::at(u32::MAX - 1, shutter, 1).phase(u32::MAX) < 1.);
        assert_eq!(SampleTime::at(u32::MAX, None, 0).phase(u32::MAX), 0.);
    }

    #[test]
    fn revising_a_parked_chart_preserves_source_geometry_and_decode() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator");
        let mut engine = Engine::new(Project::load(&root.join("chart.json"))?)?;
        let original = engine.render(90)?;
        engine.replace_project(Project::load(&root.join("chart-revised.json"))?)?;
        let revised = engine.render(90)?;
        assert_ne!(original.rgba, revised.rgba);
        assert!(revised.trace.reused_decode && revised.trace.reused_paragraph);
        assert_eq!(revised.trace.graphics.graphic_uploads, 0);
        assert_eq!(revised.trace.graphics.raster_paints, 0);
        assert_eq!(revised.trace.graphics.raster_hits, 14);
        let mut fresh = Engine::new(Project::load(&root.join("chart-revised.json"))?)?;
        assert_eq!(revised.rgba, fresh.render(90)?.rgba);
        Ok(())
    }

    #[test]
    fn a_title_backplate_hugs_the_parked_paragraph_on_reflow() -> Result<()> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/material.json");
        let mut engine = Engine::new(Project::load(&src)?)?;
        let wide = engine.render(5)?;
        engine.edit_title(None, Some(220))?;
        let narrow = engine.render(5)?;
        let a = wide.trace.backplate_rect.context("missing wide plate")?;
        let b = narrow
            .trace
            .backplate_rect
            .context("missing narrow plate")?;
        assert_eq!(a[..2], [54, 85]);
        assert_eq!(a[..2], b[..2]);
        assert!(b[2] < a[2] && b[3] > a[3]);
        assert_ne!(wide.trace.paragraph_lines, narrow.trace.paragraph_lines);
        assert!(narrow.trace.reused_decode && !narrow.trace.reused_paragraph);
        assert_eq!(narrow.trace.graphics.graphic_uploads, 0);
        let parked = engine.render(5)?;
        assert!(parked.trace.reused_paragraph);
        assert_eq!(parked.rgba, narrow.rgba);
        Ok(())
    }

    #[test]
    fn svg_import_reports_unsupported_presentation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("mask.svg");
        fs::write(
            &file,
            r##"<svg width="9" height="8"><mask><path d="M0 0L7 7"/></mask></svg>"##,
        )?;
        let error = paint_svg(&file)
            .err()
            .context("expected unsupported element")?;
        assert!(error.to_string().contains("unsupported SVG element mask"));
        Ok(())
    }
}
