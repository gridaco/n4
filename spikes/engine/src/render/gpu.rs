#[cfg(test)]
mod affine_tests;
mod backdrop;
mod background;
mod effect_layout;
mod history;
#[cfg(test)]
mod knockout_tests;
mod sampled;
mod shadow;
mod source_blur;
mod source_effects;
pub(crate) mod spatial;
#[cfg(test)]
mod timed_alpha_tests;
use effect_layout::EffectLayout;
use source_effects::{PreparedSource, SourceFeatures};

use super::affine::{LayerSpace, SampledTransform, Transform};
use super::{LayerSample, sample_layer, sample_mask};
use crate::graphics::{
    self, Blend, Content, Exposure, Graphic, ObjectMatte, PaintCache, PaintStats, SampleTime,
    SampledGrade, SourceEffect,
};
use crate::media::{DecodedFrame, FrameAlpha};
use anyhow::{Context, Result, ensure};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, mpsc},
    time::Duration,
};
use wgpu::util::DeviceExt;

/// Title-overlay bytes carry coverage premultiplied in encoded sRGB.
pub(crate) type Overlay<'a> = (&'a [u8], u32, u32, i32, i32, f32);

/// Interpretation of a single texture load, before transforms, effects or blending.
/// Values match n4_source_color in linear_premul.wgsl. sRGB texture views already
/// return linear RGB, while painter textures expose their encoded bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum TexelColor {
    LinearPremultiplied = 0,
    SrgbPremultiplied = 1,
    LinearStraight = 2,
}

impl From<FrameAlpha> for TexelColor {
    fn from(alpha: FrameAlpha) -> Self {
        match alpha {
            FrameAlpha::Opaque => Self::LinearPremultiplied,
            FrameAlpha::Straight => Self::LinearStraight,
        }
    }
}

struct UploadedFrame<'a> {
    texture: wgpu::Texture,
    frame: &'a DecodedFrame,
}

type VideoLayerKey<'a> = (&'a Path, [u32; 2], u32);

pub(crate) struct Gpu {
    adapter_name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    overlay_pipeline: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    background: RefCell<Option<background::Cached>>,
    exposure_pipeline: wgpu::RenderPipeline,
    source_history: RefCell<Option<history::Atlas>>,
    effects: RefCell<VecDeque<CachedEffect>>,
    sampled_scratch: RefCell<Option<sampled::Scratch>>,
    multiply_snapshot: RefCell<Option<wgpu::Texture>>,
    shadow_tint: wgpu::RenderPipeline,
    source_blurs: RefCell<source_blur::Cache>,
    spatial: RefCell<Option<spatial::SpatialPass>>,
    /// Present only in a build with the `skia-gpu` feature, unless switched off at run time.
    skia: Option<RefCell<super::skia_gpu::SkiaGpu>>,
}

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const IDENTITY_EFFECT: &str =
    "fn effect(p: vec2<i32>, color: vec4<f32>, amount: f32) -> vec4<f32> { return color; }";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShaderKind {
    Point {
        blend: Blend,
        features: SourceFeatures,
        intermediate: bool,
    },
    Temporal,
    LinearSource,
    Sampled,
    Backdrop2d,
    Coverage,
}

struct CachedEffect {
    kind: ShaderKind,
    body: String,
    matte_names: Vec<String>,
    pipeline: wgpu::RenderPipeline,
}

impl Gpu {
    pub(crate) fn new() -> Result<Self> {
        Self::with_painter(true)
    }

    /// Vector artwork painted on the CPU and uploaded: the reference for the device painter.
    #[cfg(test)]
    pub(crate) fn reference() -> Result<Self> {
        Self::with_painter(false)
    }

    fn with_painter(on_device: bool) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        }))
        .context("request an offscreen GPU adapter")?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("N4 compositor"),
            ..Default::default()
        }))
        .context("request the native compositor device")?;
        let overlay_pipeline = Self::make_pipeline(
            &device,
            ShaderKind::Point {
                blend: Blend::SourceOver,
                features: SourceFeatures::default(),
                intermediate: false,
            },
            IDENTITY_EFFECT,
            &[],
        )?;
        let background_pipeline = Self::compile_pipeline(
            &device,
            concat!(
                include_str!("gpu/fullscreen.wgsl"),
                "\n",
                include_str!("gpu/background.wgsl")
            ),
            TARGET_FORMAT,
            None,
            EffectLayout::Sampled(0),
        )?;
        let exposure_pipeline = Self::compile_pipeline(
            &device,
            include_str!("exposure.wgsl"),
            TARGET_FORMAT,
            None,
            EffectLayout::Inferred,
        )?;
        let shadow_tint = Self::compile_pipeline(
            &device,
            concat!(
                include_str!("gpu/fullscreen.wgsl"),
                "\n",
                include_str!("gpu/shadow.wgsl")
            ),
            TARGET_FORMAT,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            EffectLayout::Sampled(1),
        )?;
        let skia = if on_device {
            super::skia_gpu::SkiaGpu::new(&device, &queue)?.map(RefCell::new)
        } else {
            None
        };
        Ok(Self {
            skia,
            source_history: Default::default(),
            background_pipeline,
            background: RefCell::new(None),
            shadow_tint,
            spatial: Default::default(),
            source_blurs: RefCell::default(),
            exposure_pipeline,
            adapter_name,
            device,
            queue,
            overlay_pipeline,
            effects: RefCell::new(VecDeque::new()),
            sampled_scratch: RefCell::new(None),
            multiply_snapshot: RefCell::new(None),
        })
    }

    fn make_pipeline(
        device: &wgpu::Device,
        kind: ShaderKind,
        body: &str,
        mattes: &[ObjectMatte],
    ) -> Result<wgpu::RenderPipeline> {
        let (source, format) = match kind {
            ShaderKind::Point {
                blend,
                features,
                intermediate,
            } => {
                let grade = if features.grade {
                    include_str!("gpu/rgb_grade.wgsl")
                } else {
                    "fn n4_grade(color: vec4<f32>) -> vec4<f32> { return color; }"
                };
                let blur_mix = if features.blur {
                    include_str!("gpu/source_blur.wgsl")
                } else {
                    "fn n4_blur_mix(local: vec2<i32>, texel: vec2<i32>, sharp: vec4<f32>) -> vec4<f32> { return sharp; }"
                };
                let fragment = if blend == Blend::Multiply {
                    include_str!("gpu/multiply.wgsl")
                } else {
                    "@fragment fn fragment(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return n4_pixel(p); }"
                };
                let mut matte_helpers = String::new();
                if !mattes.is_empty() {
                    use std::fmt::Write;
                    matte_helpers.push_str(include_str!("gpu/object_mattes.wgsl"));
                    for (index, matte) in mattes.iter().enumerate() {
                        writeln!(
                            matte_helpers,
                            "fn matte_{}(p: vec2<i32>) -> f32 {{ return n4_matte_at(p, {index}); }}",
                            matte.name
                        )?;
                    }
                }
                (
                    format!(
                        "{}\n{}\n{blur_mix}\n{grade}\n{matte_helpers}\n{body}\n{fragment}",
                        include_str!("gpu/linear_premul.wgsl"),
                        include_str!("overlay.wgsl")
                    ),
                    if intermediate {
                        wgpu::TextureFormat::Rgba16Float
                    } else {
                        TARGET_FORMAT
                    },
                )
            }
            ShaderKind::LinearSource => (
                concat!(
                    include_str!("gpu/fullscreen.wgsl"),
                    "\n",
                    include_str!("gpu/linear_premul.wgsl"),
                    "\n",
                    include_str!("gpu/linear_source.wgsl")
                )
                .to_owned(),
                wgpu::TextureFormat::Rgba16Float,
            ),
            ShaderKind::Sampled => (
                format!(
                    "{}\n{}\n{body}",
                    include_str!("gpu/fullscreen.wgsl"),
                    include_str!("gpu/sampled.wgsl")
                ),
                wgpu::TextureFormat::Rgba16Float,
            ),
            ShaderKind::Backdrop2d => (
                format!(
                    "{}\n{}\n{body}",
                    include_str!("gpu/fullscreen.wgsl"),
                    include_str!("gpu/backdrop2d.wgsl")
                ),
                wgpu::TextureFormat::Rgba16Float,
            ),
            ShaderKind::Temporal => (
                format!(
                    "{}\n{}\n{body}",
                    include_str!("gpu/fullscreen.wgsl"),
                    include_str!("gpu/history.wgsl")
                ),
                wgpu::TextureFormat::Rgba16Float,
            ),
            ShaderKind::Coverage => (
                format!(
                    "{}\n{}\n{body}",
                    include_str!("gpu/fullscreen.wgsl"),
                    include_str!("gpu/resolve.wgsl")
                ),
                TARGET_FORMAT,
            ),
        };
        let layout = match kind {
            ShaderKind::Point {
                blend, features, ..
            } if !mattes.is_empty() => EffectLayout::ObjectMattes(blend, features),
            ShaderKind::Point { .. } => EffectLayout::Inferred,
            ShaderKind::LinearSource | ShaderKind::Sampled | ShaderKind::Backdrop2d => {
                EffectLayout::Sampled(1)
            }
            ShaderKind::Coverage => EffectLayout::Sampled(3),
            ShaderKind::Temporal => EffectLayout::History,
        };
        let blend = match kind {
            ShaderKind::Point {
                intermediate: true, ..
            } => None,
            ShaderKind::Point {
                blend: Blend::SourceOver,
                ..
            } => Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            ShaderKind::Point {
                blend: Blend::DestinationOut,
                ..
            } => {
                let erase = wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                };
                Some(wgpu::BlendState {
                    color: erase,
                    alpha: erase,
                })
            }
            _ => None,
        };
        Self::compile_pipeline(device, &source, format, blend, layout)
    }

    fn compile_pipeline(
        device: &wgpu::Device,
        shader_source: &str,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
        inputs: EffectLayout,
    ) -> Result<wgpu::RenderPipeline> {
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("N4 graphics shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let layout = inputs.create(device);
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("N4 graphics pipeline"),
            layout: layout.as_ref(),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        if let Some(error) = pollster::block_on(errors.pop()) {
            anyhow::bail!("invalid layer effect: {error}");
        }
        Ok(overlay_pipeline)
    }

    fn effect(&self, kind: ShaderKind, body: &str) -> Result<wgpu::RenderPipeline> {
        self.effect_with_mattes(kind, body, &[])
    }

    fn effect_with_mattes(
        &self,
        kind: ShaderKind,
        body: &str,
        mattes: &[ObjectMatte],
    ) -> Result<wgpu::RenderPipeline> {
        let mut effects = self.effects.borrow_mut();
        let hit = effects.iter().position(|entry| {
            entry.kind == kind
                && entry.body == body
                && entry.matte_names.iter().eq(mattes.iter().map(|m| &m.name))
        });
        if let Some(index) = hit {
            let entry = effects.remove(index).expect("index came from the cache");
            let pipeline = entry.pipeline.clone();
            effects.push_back(entry);
            return Ok(pipeline);
        }
        let pipeline = Self::make_pipeline(&self.device, kind, body, mattes)?;
        if effects.len() == 32 {
            effects.pop_front();
        }
        effects.push_back(CachedEffect {
            kind,
            body: body.to_owned(),
            matte_names: mattes.iter().map(|m| m.name.clone()).collect(),
            pipeline: pipeline.clone(),
        });
        Ok(pipeline)
    }

    fn point_pipeline(
        &self,
        blend: Blend,
        shader: &str,
        features: SourceFeatures,
        mattes: &[ObjectMatte],
    ) -> Result<wgpu::RenderPipeline> {
        if features == SourceFeatures::default()
            && blend == Blend::SourceOver
            && shader == IDENTITY_EFFECT
            && mattes.is_empty()
        {
            Ok(self.overlay_pipeline.clone())
        } else {
            self.effect_with_mattes(
                ShaderKind::Point {
                    blend,
                    features,
                    intermediate: false,
                },
                shader,
                mattes,
            )
        }
    }

    pub(crate) fn validate_effects(
        &self,
        layers: &[Graphic],
        cache: &mut PaintCache,
        size: [u32; 2],
    ) -> Result<()> {
        for layer in layers {
            let mattes = layer
                .filter
                .as_ref()
                .map_or(&[][..], |f| f.mattes.as_slice());
            spatial::matte_scratch_bytes(
                size,
                mattes.len(),
                mattes.iter().any(|m| m.owner().is_some()),
            )?;
            let features = SourceFeatures {
                blur: layer
                    .effects
                    .iter()
                    .any(|e| matches!(e, SourceEffect::SourceBlur(_))),
                grade: layer
                    .effects
                    .iter()
                    .any(|e| matches!(e, SourceEffect::RgbGrade(_))),
            };
            let body = match &layer.filter {
                Some(effect) => {
                    let body = cache.shader(&effect.shader)?;
                    // A neutral animated effect can remove all optional inputs.
                    // Validate the custom shader against the plain point ABI too.
                    self.point_pipeline(Blend::SourceOver, body, SourceFeatures::default(), mattes)
                        .with_context(|| {
                            format!("compile source effect {}", effect.shader.display())
                        })?;
                    body
                }
                None => IDENTITY_EFFECT,
            };
            self.point_pipeline(layer.blend, body, features, mattes)
                .context("compile source effect stack")?;
            if layer.blend == Blend::Multiply
                && (layer.filter.is_some() || layer.underlay.is_some())
            {
                self.point_pipeline(Blend::SourceOver, body, features, mattes)?;
            }
            if features.blur {
                self.effect(ShaderKind::Sampled, source_blur::GAUSSIAN)?;
                self.effect(ShaderKind::LinearSource, "")?;
            }
            if let Content::Footage {
                history: Some(history),
            } = &layer.content
            {
                history::check_budget(size, history.frames)?;
                self.effect(ShaderKind::Temporal, cache.shader(&history.shader)?)
                    .with_context(|| {
                        format!("compile source-history shader {}", history.shader.display())
                    })?;
            }
            if let Some(underlay) = &layer.underlay {
                self.effect(ShaderKind::Sampled, cache.shader(&underlay.shader)?)
                    .with_context(|| {
                        format!("compile sampled filter {}", underlay.shader.display())
                    })?;
            }
            if let Content::Backdrop { filter } = &layer.content {
                self.effect(
                    backdrop::shader_kind(filter),
                    cache.shader(filter.shader())?,
                )
                .with_context(|| format!("compile sampled filter {}", filter.shader().display()))?;
                let shader = match layer.mask.as_ref().and_then(|m| m.shader.as_ref()) {
                    Some(anim) => cache.shader(&anim.shader)?,
                    None => backdrop::STATIC_COVERAGE,
                };
                self.effect(ShaderKind::Coverage, shader)
                    .context("compile backdrop mask shader")?;
            }
            if let Content::Group { children } | Content::TransformGroup { children } =
                &layer.content
            {
                self.validate_effects(children, cache, size)?;
            } else if let Content::Scene3d(scene) = &layer.content {
                for surface in scene.models.iter().flat_map(|m| &m.surfaces) {
                    self.validate_effects(&surface.layers, cache, surface.size)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn allocation_probe(&self) -> impl Fn() -> Option<u64> + Send + 'static {
        #[cfg(target_os = "macos")]
        let device = self.device.clone();
        move || {
            #[cfg(target_os = "macos")]
            {
                use objc2_metal::MTLDevice;
                // Only read Metal's allocation counter while the wgpu HAL guard is alive.
                let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
                Some(hal.raw_device().currentAllocatedSize() as u64)
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        }
    }

    pub(crate) fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    #[cfg(test)]
    fn compose(
        &self,
        width: u32,
        height: u32,
        video: &[u8],
        overlay: Option<Overlay<'_>>,
    ) -> Result<Vec<u8>> {
        Ok(self
            .compose_scene(
                [width, height],
                video,
                overlay,
                &[],
                0,
                &mut PaintCache::default(),
            )?
            .0)
    }

    #[cfg(test)]
    fn compose_scene(
        &self,
        size: [u32; 2],
        video: &[u8],
        overlay: Option<Overlay<'_>>,
        graphics: &[Graphic],
        frame: u32,
        cache: &mut PaintCache,
    ) -> Result<(Vec<u8>, PaintStats)> {
        self.compose_exposed(
            FrameInputs {
                size,
                primary: PrimaryPicture::Video(video),
                video_layers: &HashMap::new(),
                overlay,
                scopes: vec![(graphics, frame)],
            },
            None,
            cache,
        )
    }

    #[cfg(test)]
    pub(crate) fn compose_exposed(
        &self,
        inputs: FrameInputs<'_>,
        exposure: Option<Exposure>,
        cache: &mut PaintCache,
    ) -> Result<(Vec<u8>, PaintStats)> {
        self.compose_view(inputs, exposure, None, cache)
    }

    pub(crate) fn compose_view(
        &self,
        inputs: FrameInputs<'_>,
        exposure: Option<Exposure>,
        camera: Option<&crate::camera::Camera>,
        cache: &mut PaintCache,
    ) -> Result<(Vec<u8>, PaintStats)> {
        let frame = self.encode_composition(inputs, exposure, camera, cache);
        if frame.is_err() {
            // A later drawable can fail before recorded convolution commands
            // are submitted. Never reuse those provisional textures on retry.
            self.source_blurs.borrow_mut().clear_results();
            self.source_history.borrow_mut().take();
        }
        frame
    }

    fn encode_composition(
        &self,
        inputs: FrameInputs<'_>,
        exposure: Option<Exposure>,
        camera: Option<&crate::camera::Camera>,
        cache: &mut PaintCache,
    ) -> Result<(Vec<u8>, PaintStats)> {
        let FrameInputs {
            size: [width, height],
            primary,
            video_layers,
            overlay,
            mut scopes,
        } = inputs;
        let count = exposure.map_or(1, |e| u32::from(e.samples));
        ensure!(
            exposure.is_none()
                || u64::from(width) * u64::from(height) * 4 * u64::from(count) <= 256 * 1024 * 1024,
            "temporal sample array exceeds the 256 MiB study limit"
        );
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING;
        let mut commands = self.device.create_command_encoder(&Default::default());
        let immutable_plate = matches!(primary, PrimaryPicture::Background(_));
        let video_frame = match primary {
            PrimaryPicture::Decoded(frame) | PrimaryPicture::DecodedHistory(frame, _) => {
                Some(frame)
            }
            _ => None,
        };
        let plate_color =
            video_frame.map_or(TexelColor::LinearPremultiplied, |frame| frame.alpha.into());
        let transparent_plate = plate_color == TexelColor::LinearStraight;
        let video = match primary {
            PrimaryPicture::Decoded(frame) | PrimaryPicture::DecodedHistory(frame, _) => {
                self.upload(width, height, &frame.rgba, TARGET_FORMAT, usage)?
            }
            #[cfg(test)]
            PrimaryPicture::Video(bytes) => {
                self.upload(width, height, bytes, TARGET_FORMAT, usage)?
            }
            PrimaryPicture::Background(material) => {
                self.background_texture([width, height], material)?
            }
        };
        let history = match primary {
            PrimaryPicture::DecodedHistory(frame, previous) => {
                Some(self.prepare_history(&mut commands, &video, &frame.rgba, previous)?)
            }
            _ => None,
        };
        let mut video_textures = HashMap::with_capacity(video_layers.len());
        for (&key, frame) in video_layers {
            let (_, [w, h], _) = key;
            let texture = self.upload(
                w,
                h,
                &frame.rgba,
                TARGET_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING,
            )?;
            video_textures.insert(key, UploadedFrame { texture, frame });
        }
        // Use a separate output when sampling the plate, or when the pristine
        // input has to be restored before the next exposure sample.
        // A known opaque plate with a plain backmost placement is already
        // present in that texture, so it can seed the scope without a fullscreen draw.
        let camera_plate =
            !immutable_plate && matches!(camera, Some(crate::camera::Camera::Orthographic { .. }));
        let draw_plate = camera_plate || transparent_plate;
        let first_scope = scopes.iter_mut().find(|(layers, _)| !layers.is_empty());
        let backmost_plate = !draw_plate
            && first_scope.as_ref().is_some_and(|(layers, clock)| {
                let layer = &layers[0];
                matches!(layer.content, Content::Footage { history: None })
                    && layer.placed_position() == [0, 0]
                    && layer.transform.is_none_or(Transform::is_identity)
                    && !layer.has_motion()
                    && layer.span.is_none()
                    && layer.blend == Blend::SourceOver
                    && layer.mask.is_none()
                    && layer.filter.is_none()
                    && layer.effects.iter().all(|effect| {
                        (0..count).all(|sample| {
                            let fraction = SampleTime::at(*clock, exposure, sample).fraction();
                            match effect {
                                SourceEffect::SourceBlur(blur) => {
                                    let value = blur.sample(*clock, fraction);
                                    value.sigma == 0. || value.amount == 0.
                                }
                                SourceEffect::RgbGrade(grade) => {
                                    grade.sample(*clock, fraction) == SampledGrade::IDENTITY
                                }
                            }
                        })
                    })
                    && layer.wipe.is_none()
                    && layer.underlay.is_none()
            });
        if backmost_plate && let Some((layers, _)) = first_scope {
            *layers = &layers[1..];
        }
        let samples_original = scopes
            .iter()
            .any(|(layers, _)| graphics::uses_footage(layers));
        let seed_with_plate = backmost_plate || !samples_original;
        let paints = scopes.iter().any(|(layers, _)| !layers.is_empty()) || overlay.is_some();
        let separate_target =
            draw_plate || samples_original || exposure.is_some() || (immutable_plate && paints);
        let target = if separate_target {
            self.render_texture("composited picture", video.size())
        } else {
            video.clone()
        };
        let samples = exposure.map(|_| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("temporal sample array"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: count,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                view_formats: &[],
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            })
        });
        let title = overlay
            .map(|(rgba, w, h, x, y, opacity)| -> Result<_> {
                let tex = self.upload(
                    w,
                    h,
                    rgba,
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureUsages::TEXTURE_BINDING,
                )?;
                Ok((tex, [x, y], opacity))
            })
            .transpose()?;
        let mut stats = PaintStats {
            temporal_samples: count,
            history_uploads: history.as_ref().map_or(0, |window| window.uploads),
            ..Default::default()
        };
        for index in 0..count {
            let shot_time = SampleTime::at(
                scopes.first().map_or(0, |(_, local)| *local),
                exposure,
                index,
            );
            let view_transform =
                camera.and_then(|camera| camera.affine(shot_time, [width, height]));
            if draw_plate && seed_with_plate {
                Self::clear(&mut commands, &target, wgpu::Color::BLACK);
                self.paint(
                    &mut commands,
                    &target,
                    &video,
                    Draw {
                        blend: Blend::SourceOver,
                        transform: view_transform,
                        destination_art: false,
                        origin: [0; 2],
                        artwork_origin: [0; 2],
                        mask: None,
                        wipe_right: None,
                        opacity: 1.,
                        amount: 0.,
                        color: plate_color,
                        blur: None,
                        grades: &[],
                        object_mattes: None,
                        pipeline: &self.overlay_pipeline,
                    },
                    wgpu::LoadOp::Load,
                )?;
            } else if separate_target && seed_with_plate {
                commands.copy_texture_to_texture(
                    video.as_image_copy(),
                    target.as_image_copy(),
                    video.size(),
                );
            } else if separate_target {
                Self::clear(&mut commands, &target, wgpu::Color::BLACK);
            }
            for (scope, &(layers, clock)) in scopes.iter().enumerate() {
                let mut painter = ScenePainter {
                    perspective: if scope == 0 {
                        camera.and_then(|c| c.perspective(shot_time))
                    } else {
                        None
                    },
                    gpu: self,
                    commands: &mut commands,
                    history: history.as_ref(),
                    video: &video,
                    video_layers: &video_textures,
                    video_frame,
                    time: SampleTime::at(clock, exposure, index),
                    cache,
                    stats: &mut stats,
                };
                painter.layers(
                    layers,
                    &target,
                    [0, 0],
                    if scope == 0 { view_transform } else { None },
                    clock,
                )?;
            }
            if let Some((art, position, opacity)) = &title {
                self.paint(
                    &mut commands,
                    &target,
                    art,
                    Draw {
                        blend: Blend::SourceOver,
                        transform: None,
                        destination_art: false,
                        origin: position.map(i64::from),
                        artwork_origin: position.map(i64::from),
                        mask: None,
                        wipe_right: None,
                        opacity: *opacity,
                        amount: 0.,
                        color: TexelColor::SrgbPremultiplied,
                        blur: None,
                        grades: &[],
                        object_mattes: None,
                        pipeline: &self.overlay_pipeline,
                    },
                    wgpu::LoadOp::Load,
                )?;
            }
            if let Some(array) = &samples {
                let mut dst = array.as_image_copy();
                dst.origin.z = index;
                commands.copy_texture_to_texture(target.as_image_copy(), dst, target.size());
            }
        }
        if let Some(array) = samples {
            let source = array.create_view(&Default::default());
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("temporal samples"),
                layout: &self.exposure_pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source),
                }],
            });
            let view = target.create_view(&Default::default());
            let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("resolve temporal exposure"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.exposure_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok((self.readback(commands, &target)?, stats))
    }

    fn blank(
        &self,
        commands: &mut wgpu::CommandEncoder,
        template: &wgpu::Texture,
        clear: wgpu::Color,
    ) -> wgpu::Texture {
        let texture = self.render_texture("isolated layer group", template.size());
        Self::clear(commands, &texture, clear);
        texture
    }

    fn render_texture(&self, label: &str, size: wgpu::Extent3d) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            view_formats: &[],
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
        })
    }

    fn clear(commands: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, clear: wgpu::Color) {
        let view = texture.create_view(&Default::default());
        let _clear = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }

    fn upload(
        &self,
        width: u32,
        height: u32,
        bytes: &[u8],
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Result<wgpu::Texture> {
        let limit = self.device.limits().max_texture_dimension_2d;
        ensure!(
            width > 0 && height > 0 && width <= limit && height <= limit,
            "image dimensions {width} × {height} exceed the supported range 1..={limit}"
        );
        ensure!(
            bytes.len() as u64 == u64::from(width) * u64::from(height) * 4,
            "wrong RGBA8 byte count for {width} × {height}: {}",
            bytes.len()
        );
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("compositor input"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: usage | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            tex.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            size,
        );
        Ok(tex)
    }

    /// Copy only the scissored destination. Reuse the texture across layers,
    /// including consecutive overlapping marks in the same command buffer.
    fn copy_multiply_destination(
        &self,
        commands: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        bounds: Bounds,
    ) -> wgpu::TextureView {
        let size = wgpu::Extent3d {
            width: bounds.width(),
            height: bounds.height(),
            depth_or_array_layers: 1,
        };
        let mut slot = self.multiply_snapshot.borrow_mut();
        if slot
            .as_ref()
            .is_none_or(|texture| texture.width() < size.width || texture.height() < size.height)
        {
            // Growing animated bounds should rarely require a new allocation.
            // Copy only the requested rectangle regardless of the reserved size.
            let capacity = wgpu::Extent3d {
                width: size
                    .width
                    .next_power_of_two()
                    .min(target.width())
                    .max(slot.as_ref().map_or(0, |t| t.width())),
                height: size
                    .height
                    .next_power_of_two()
                    .min(target.height())
                    .max(slot.as_ref().map_or(0, |t| t.height())),
                ..size
            };
            *slot = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Multiply destination snapshot"),
                size: capacity,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
        }
        let snapshot = slot.as_ref().expect("Multiply snapshot reserved");
        let mut source = target.as_image_copy();
        source.origin.x = bounds.left as u32;
        source.origin.y = bounds.top as u32;
        commands.copy_texture_to_texture(source, snapshot.as_image_copy(), size);
        snapshot.create_view(&Default::default())
    }

    fn paint(
        &self,
        commands: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        artwork: &wgpu::Texture,
        draw: Draw<'_>,
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Result<u64> {
        ensure!(
            (0.0..=1.0).contains(&draw.opacity),
            "opacity must be between 0 and 1"
        );
        let [x, y] = draw.artwork_origin;
        let bounds = draw
            .bounds(artwork)
            .intersect(Bounds::texture(target, [0; 2]));
        if bounds.empty() || draw.opacity == 0. {
            return Ok(0);
        }
        let blur_params = draw.blur.map_or([0.; 4], |(_, params)| {
            let [center, half_width, feather] = params.focus_band.unwrap_or([0.; 3]);
            [params.amount, center, half_width, feather]
        });
        let [mx, my] = draw.mask.map_or([0; 2], |(_, position)| position);
        let transform = draw.transform.unwrap_or(SampledTransform::IDENTITY);
        let [inverse_x, inverse_y] = transform.inverse_rows();
        let [dx, dy] = transform.map_point([0.; 2]);
        let origin = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("layer uniforms"),
                contents: &[
                    (draw.origin[0] as i32).to_le_bytes(),
                    (draw.origin[1] as i32).to_le_bytes(),
                    (x as i32).to_le_bytes(),
                    (y as i32).to_le_bytes(),
                    (mx as i32).to_le_bytes(),
                    (my as i32).to_le_bytes(),
                    draw.opacity.to_le_bytes(),
                    draw.amount.to_le_bytes(),
                    (draw.color as u32).to_le_bytes(),
                    u32::from(draw.mask.is_some()).to_le_bytes(),
                    u32::from(draw.transform.is_some()).to_le_bytes(),
                    draw.wipe_right
                        .map_or(i32::MAX, |x| (x - draw.origin[0]) as i32)
                        .to_le_bytes(),
                    (inverse_x[0] as f32).to_le_bytes(),
                    (inverse_x[1] as f32).to_le_bytes(),
                    (inverse_y[0] as f32).to_le_bytes(),
                    (inverse_y[1] as f32).to_le_bytes(),
                    ((draw.origin[0] as f64 + dx) as f32).to_le_bytes(),
                    ((draw.origin[1] as f64 + dy) as f32).to_le_bytes(),
                    u32::from(draw.destination_art).to_le_bytes(),
                    0u32.to_le_bytes(),
                    (bounds.left as i32).to_le_bytes(),
                    (bounds.top as i32).to_le_bytes(),
                    0u32.to_le_bytes(), // Align blur_params to WGSL vec4.
                    0u32.to_le_bytes(),
                    blur_params[0].to_le_bytes(),
                    blur_params[1].to_le_bytes(),
                    blur_params[2].to_le_bytes(),
                    blur_params[3].to_le_bytes(),
                ]
                .concat(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let view = artwork.create_view(&Default::default());
        let mask_view = draw
            .mask
            .map(|(mask, _)| mask.create_view(&Default::default()));
        let snapshot = (draw.blend == Blend::Multiply)
            .then(|| self.copy_multiply_destination(commands, target, bounds));
        let blurred_view = draw
            .blur
            .map(|(tex, _)| tex.create_view(&Default::default()));
        let grade_buffer = (!draw.grades.is_empty()).then(|| {
            // WGSL vec3 members have 16-byte alignment and a 32-byte struct stride.
            let bytes: Vec<u8> = draw
                .grades
                .iter()
                .flat_map(|g| {
                    [
                        g.gain[0],
                        g.gain[1],
                        g.gain[2],
                        0.,
                        g.offset[0],
                        g.offset[1],
                        g.offset[2],
                        0.,
                    ]
                })
                .flat_map(f32::to_le_bytes)
                .collect();
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ordered RGB grade parameters"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                })
        });
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: origin.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(mask_view.as_ref().unwrap_or(&view)),
            },
        ];
        for (binding, input) in [(3, snapshot.as_ref()), (4, blurred_view.as_ref())] {
            if let Some(view) = input {
                entries.push(wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
        }
        if let Some(buffer) = &grade_buffer {
            entries.push(wgpu::BindGroupEntry {
                binding: 5,
                resource: buffer.as_entire_binding(),
            });
        }
        let matte_view = draw.object_mattes.map(|texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        });
        if let Some(view) = &matte_view {
            entries.push(wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer source"),
            layout: &draw.pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let target_view = target.create_view(&Default::default());
        let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_scissor_rect(
            bounds.left as u32,
            bounds.top as u32,
            bounds.width(),
            bounds.height(),
        );
        pass.set_pipeline(draw.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        Ok(u64::from(bounds.width()) * u64::from(bounds.height()))
    }

    /// Copy a finished RGBA8 texture to memory, outside any frame in progress.
    pub(crate) fn read_texture(&self, texture: &wgpu::Texture) -> Result<Vec<u8>> {
        self.readback(
            self.device.create_command_encoder(&Default::default()),
            texture,
        )
    }

    fn readback(
        &self,
        mut commands: wgpu::CommandEncoder,
        target: &wgpu::Texture,
    ) -> Result<Vec<u8>> {
        let row = target.width() * 4;
        let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let size = u64::from(stride) * u64::from(target.height());
        ensure!(
            size <= self.device.limits().max_buffer_size,
            "frame readback requires {size} bytes, which exceeds the device limit"
        );
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("padded frame readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        commands.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        let submission = self.queue.submit([commands.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(30)),
            })
            .context("wait for offscreen composition")?;
        rx.recv_timeout(Duration::from_secs(1))
            .context("wait for frame mapping")?
            .context("map finished frame")?;
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .context("access finished frame")?;
        let mut rgba = Vec::with_capacity(row as usize * target.height() as usize);
        for padded in mapped.chunks_exact(stride as usize) {
            rgba.extend_from_slice(&padded[..row as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(rgba)
    }
}

pub(crate) struct FrameInputs<'a> {
    pub size: [u32; 2],
    pub primary: PrimaryPicture<'a>,
    pub video_layers: &'a HashMap<VideoLayerKey<'a>, DecodedFrame>,
    pub overlay: Option<Overlay<'a>>,
    /// Ordered artwork and the frame in each owning shot or placement.
    pub scopes: Vec<(&'a [Graphic], u32)>,
}

pub(crate) enum PrimaryPicture<'a> {
    #[cfg(test)]
    Video(&'a [u8]),
    Decoded(&'a DecodedFrame),
    DecodedHistory(&'a DecodedFrame, &'a [Option<Arc<Vec<u8>>>]),
    Background(&'a crate::project::Background),
}

#[derive(Clone, Copy)]
struct Draw<'a> {
    blend: Blend,
    // Children of an isolated group have already painted their transformed
    // pixels into a destination-sized image. Its mask and point effects still
    // evaluate in the authored coordinates of the group.
    destination_art: bool,
    transform: Option<SampledTransform>,
    origin: [i64; 2],
    artwork_origin: [i64; 2],
    mask: Option<(&'a wgpu::Texture, [i64; 2])>,
    wipe_right: Option<i64>,
    opacity: f32,
    amount: f32,
    color: TexelColor,
    blur: Option<(&'a wgpu::Texture, graphics::SampledSourceBlur)>,
    grades: &'a [SampledGrade],
    object_mattes: Option<&'a wgpu::Texture>,
    pipeline: &'a wgpu::RenderPipeline,
}

impl Draw<'_> {
    fn bounds(self, artwork: &wgpu::Texture) -> Bounds {
        let art = Bounds::texture(artwork, self.artwork_origin);
        if self.destination_art {
            let Some((mask, origin)) = self.mask else {
                return art;
            };
            let mut bounds = Bounds::texture(mask, origin);
            if let Some(edge) = self.wipe_right {
                bounds.right = bounds.right.min(edge);
            }
            return art.intersect(bounds.transformed(self.origin, self.transform));
        }
        art.clip_source(
            self.mask.map(|(tex, pos)| Bounds::texture(tex, pos)),
            self.wipe_right,
        )
        .transformed(self.origin, self.transform)
    }
}

// Signed composition coordinates include casters displaced past the output edge.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Bounds {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}
impl Bounds {
    // A descendant can leave signed bounds and a later parent can bring it
    // back. Preserve the conservative envelope until the destination clip.
    const UNBOUNDED: Self = Self {
        left: i64::MIN,
        top: i64::MIN,
        right: i64::MAX,
        bottom: i64::MAX,
    };
    fn new([x, y]: [i64; 2], [w, h]: [u32; 2]) -> Self {
        Self {
            left: x,
            top: y,
            right: x + i64::from(w),
            bottom: y + i64::from(h),
        }
    }
    fn texture(tex: &wgpu::Texture, origin: [i64; 2]) -> Self {
        Self::new(origin, [tex.width(), tex.height()])
    }
    fn clip_source(mut self, mask: Option<Self>, wipe_right: Option<i64>) -> Self {
        if let Some(edge) = wipe_right {
            self.right = self.right.min(edge);
        }
        mask.map_or(self, |bounds| self.intersect(bounds))
    }
    fn empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
    fn transformed(self, origin: [i64; 2], transform: Option<SampledTransform>) -> Self {
        let Some(t) = transform else { return self };
        if self.empty() || self == Self::UNBOUNDED {
            return self;
        }
        let [ox, oy] = origin.map(|v| v as f64);
        let mut left = f64::INFINITY;
        let mut top = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        // The linear reconstruction kernel has support half a source texel
        // beyond the outer edges of the artwork, wipe and mask intersection.
        for x in [self.left as f64 - ox - 0.5, self.right as f64 - ox + 0.5] {
            for y in [self.top as f64 - oy - 0.5, self.bottom as f64 - oy + 0.5] {
                let [x, y] = t.map_point([x, y]);
                let (dx, dy) = (ox + x, oy + y);
                left = left.min(dx);
                top = top.min(dy);
                right = right.max(dx);
                bottom = bottom.max(dy);
            }
        }
        Self::from_extents(left, top, right, bottom)
    }
    fn from_extents(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        if left <= i64::MIN as f64
            || top <= i64::MIN as f64
            || right >= i64::MAX as f64
            || bottom >= i64::MAX as f64
        {
            return Self::UNBOUNDED;
        }
        Self {
            left: left.floor() as i64,
            top: top.floor() as i64,
            right: right.ceil() as i64,
            bottom: bottom.ceil() as i64,
        }
    }
    /// Bound independently attainable translations, scales and angles before
    /// applying the next ancestor. Angle extrema in screen space can occur
    /// strictly between the authored keys.
    fn motion_envelope(self, origin: [i64; 2], layer: &Graphic) -> Self {
        if self.empty() || self == Self::UNBOUNDED {
            return self;
        }
        if layer.rotation_varies() {
            return self.rotating_envelope(origin, layer);
        }
        let animated =
            !layer.motion.is_empty() || layer.channels.as_ref().is_some_and(|c| c.has_spatial());
        let bounds_at = |pose| {
            let t = SampledTransform::new(layer.transform, pose);
            // Immediately beside an identity key the filtered draw regains its
            // half-texel support. Include that limiting footprint in admission.
            let t = t.or_else(|| animated.then_some(SampledTransform::IDENTITY));
            self.transformed(origin, t)
        };
        let mut keys = layer.spatial_extrema();
        let first = bounds_at(keys.next().unwrap_or_default());
        keys.fold(first, |a, key| {
            let b = bounds_at(key);
            Self {
                left: a.left.min(b.left),
                top: a.top.min(b.top),
                right: a.right.max(b.right),
                bottom: a.bottom.max(b.bottom),
            }
        })
    }
    fn rotating_envelope(self, origin: [i64; 2], layer: &Graphic) -> Self {
        let base = layer.transform.unwrap_or_default();
        let static_scale = base.scales();
        let mut min = [f64::INFINITY; 5];
        let mut max = [f64::NEG_INFINITY; 5];
        for pose in layer.spatial_extrema() {
            let [sx, sy] = pose.scales();
            let values = [
                f64::from(pose.x),
                f64::from(pose.y),
                static_scale[0] * sx,
                static_scale[1] * sy,
                f64::from(pose.rotation),
            ];
            for (i, v) in values.into_iter().enumerate() {
                min[i] = min[i].min(v);
                max[i] = max[i].max(v);
            }
        }
        let px = origin[0] as f64 + f64::from(base.pivot[0]);
        let py = origin[1] as f64 + f64::from(base.pivot[1]);
        let start = (f64::from(base.rotation_degrees.unwrap_or_default()).rem_euclid(360.)
            + min[4].rem_euclid(360.))
        .to_radians();
        let span = (max[4] - min[4]).min(360.).to_radians();
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        // For a given angle, each output axis is linear in each scaled corner
        // coordinate, so four extrema of the scaled rectangle suffice. As that
        // corner rotates, each projection has extrema at the arc endpoints or
        // at a cardinal heading traversed during the sweep.
        let scaled =
            |lo: f64, hi: f64, s0: f64, s1: f64| [(lo * s0).min(lo * s1), (hi * s0).max(hi * s1)];
        let xs = scaled(
            self.left as f64 - px - 0.5,
            self.right as f64 - px + 0.5,
            min[2],
            max[2],
        );
        let ys = scaled(
            self.top as f64 - py - 0.5,
            self.bottom as f64 - py + 0.5,
            min[3],
            max[3],
        );
        for x in xs {
            for y in ys {
                use std::f64::consts::{FRAC_PI_2, TAU};
                let radius = x.hypot(y);
                let from = start + y.atan2(x);
                for a in [from, from + span] {
                    let (sin, cos) = a.sin_cos();
                    for (i, v) in [radius * cos, radius * sin].into_iter().enumerate() {
                        low[i] = low[i].min(v);
                        high[i] = high[i].max(v);
                    }
                }
                for quarter in 0..4 {
                    let delta = (f64::from(quarter) * FRAC_PI_2 - from).rem_euclid(TAU);
                    if delta <= span {
                        match quarter {
                            0 => high[0] = high[0].max(radius),
                            1 => high[1] = high[1].max(radius),
                            2 => low[0] = low[0].min(-radius),
                            _ => low[1] = low[1].min(-radius),
                        }
                    }
                }
            }
        }
        Self::from_extents(
            px + min[0] + low[0],
            py + min[1] + low[1],
            px + max[0] + high[0],
            py + max[1] + high[1],
        )
    }
    fn through_camera(self, size: [u32; 2], ranges: Option<crate::camera::OrthoRanges>) -> Self {
        let Some([cx, cy, zoom, roll]) = ranges else {
            return self;
        };
        if self.empty() || self == Self::UNBOUNDED {
            return self;
        }
        let [ox, oy] = size.map(|v| f64::from(v) * 0.5);
        let axis = |lo: i64, hi: i64, center: [f32; 2]| {
            let lo = lo as f64 - 0.5 - f64::from(center[1]);
            let hi = hi as f64 + 0.5 - f64::from(center[0]);
            let [z0, z1] = zoom.map(f64::from);
            [(lo * z0).min(lo * z1), (hi * z0).max(hi * z1)]
        };
        let xs = axis(self.left, self.right, cx);
        let ys = axis(self.top, self.bottom, cy);
        if roll[0] != roll[1] {
            // The largest corner radius encloses every intermediate angle.
            let r = xs[0]
                .abs()
                .max(xs[1].abs())
                .hypot(ys[0].abs().max(ys[1].abs()));
            return Self::from_extents(ox - r, oy - r, ox + r, oy + r);
        }
        let (sin, cos) = (-f64::from(roll[0])).to_radians().sin_cos();
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for x in xs {
            for y in ys {
                for (i, v) in [ox + cos * x - sin * y, oy + sin * x + cos * y]
                    .into_iter()
                    .enumerate()
                {
                    lo[i] = lo[i].min(v);
                    hi[i] = hi[i].max(v);
                }
            }
        }
        Self::from_extents(lo[0], lo[1], hi[0], hi[1])
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
    fn width(self) -> u32 {
        (self.right - self.left) as u32
    }
    fn height(self) -> u32 {
        (self.bottom - self.top) as u32
    }
}

fn ancestor_envelope(bounds: Bounds, ancestors: &[([i64; 2], &Graphic)]) -> Bounds {
    ancestors.iter().rev().fold(bounds, |b, (origin, layer)| {
        b.motion_envelope(*origin, layer)
    })
}

struct ScenePainter<'a> {
    perspective: Option<crate::camera::PerspectiveView>,
    gpu: &'a Gpu,
    history: Option<&'a history::Window>,
    commands: &'a mut wgpu::CommandEncoder,
    video: &'a wgpu::Texture,
    video_frame: Option<&'a DecodedFrame>,
    video_layers: &'a HashMap<VideoLayerKey<'a>, UploadedFrame<'a>>,
    time: SampleTime,
    cache: &'a mut PaintCache,
    stats: &'a mut PaintStats,
}

impl ScenePainter<'_> {
    fn raster_texture(&mut self, content: &Content) -> Result<(wgpu::Texture, [i32; 2])> {
        if let Some(art) = self.cache.textures.get(content) {
            self.stats.raster_hits += 1;
            return Ok(art.clone());
        }
        if let Some(skia) = &self.gpu.skia
            && let Some(art) = self
                .cache
                .draw_static_on(content, &mut *skia.borrow_mut())?
        {
            self.cache.textures.insert(content.clone(), art.clone());
            self.stats.raster_paints += 1;
            self.stats.graphic_uploads += 1;
            return Ok(art);
        }
        let raster = self.cache.raster(content, self.stats)?;
        let texture = self.gpu.upload(
            raster.size[0],
            raster.size[1],
            &raster.rgba,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING,
        )?;
        let art = (texture, raster.origin);
        self.cache.textures.insert(content.clone(), art.clone());
        self.stats.graphic_uploads += 1;
        Ok(art)
    }

    fn sampled_texture(
        &mut self,
        content: &Content,
        text: Option<(&[graphics::TextAnimator], u32, f64)>,
    ) -> Result<(wgpu::Texture, [i32; 2])> {
        let mut skia = self.gpu.skia.as_ref().map(RefCell::borrow_mut);
        self.cache
            .sample_texture(content, text, self.stats, skia.as_deref_mut(), |raster| {
                self.gpu.upload(
                    raster.size[0],
                    raster.size[1],
                    &raster.rgba,
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureUsages::TEXTURE_BINDING,
                )
            })
    }

    fn mask_texture(
        &mut self,
        mask: &graphics::Mask,
        frame: u32,
    ) -> Result<(wgpu::Texture, [i32; 2])> {
        if let Some(content) = sample_mask(mask, frame, self.time.fraction()) {
            self.sampled_texture(&content, None)
        } else {
            self.raster_texture(&mask.as_content())
        }
    }

    fn layers(
        &mut self,
        layers: &[Graphic],
        target: &wgpu::Texture,
        parent: [i64; 2],
        parent_transform: Option<SampledTransform>,
        parent_frame: u32,
    ) -> Result<()> {
        for layer in layers {
            let parent_space = LayerSpace {
                origin: parent,
                transform: parent_transform,
            };
            let Some(LayerSample {
                frame,
                opacity,
                space,
            }) = sample_layer(layer, parent_space, parent_frame, self.time.fraction())
            else {
                continue;
            };
            if opacity == 0. {
                continue;
            }
            let Some(content) = layer.content_at(frame, self.time.fraction()) else {
                continue;
            };
            let LayerSpace { origin, transform } = space;
            let mattes = layer
                .filter
                .as_ref()
                .map_or(&[][..], |f| f.mattes.as_slice());
            let mut object_mattes = None;
            let (source, artwork_origin, color) = match content.as_ref() {
                Content::TransformGroup { children } => {
                    self.layers(children, target, origin, transform, frame)?;
                    continue;
                }
                Content::Backdrop { filter } => {
                    self.backdrop(layer, filter, target, origin, transform, frame)?;
                    continue;
                }
                Content::Scene3d(scene) => {
                    let (beauty, masks) = self.spatial_source(scene, target, frame, mattes)?;
                    object_mattes = masks;
                    (beauty, origin, TexelColor::LinearPremultiplied)
                }
                Content::Footage { history: None } => (
                    self.video.clone(),
                    origin,
                    self.video_frame
                        .map_or(TexelColor::LinearPremultiplied, |frame| frame.alpha.into()),
                ),
                Content::Footage {
                    history: Some(history),
                } => (
                    self.temporal_source(history)?,
                    origin,
                    TexelColor::LinearPremultiplied,
                ),
                Content::Video(clip) => {
                    let Some(tick) = clip.source_frame(self.time.frame) else {
                        continue;
                    };
                    let key = (clip.source.as_path(), clip.size, tick);
                    let uploaded = self
                        .video_layers
                        .get(&key)
                        .context("missing decoded frame for active video layer")?;
                    (
                        uploaded.texture.clone(),
                        origin,
                        uploaded.frame.alpha.into(),
                    )
                }
                Content::Group { children } => {
                    // Isolate in destination coordinates. A child can be at a
                    // negative group-local position and still be on screen.
                    let group = self
                        .gpu
                        .blank(self.commands, target, wgpu::Color::TRANSPARENT);
                    self.layers(children, &group, origin, transform, frame)?;
                    (group, [0; 2], TexelColor::LinearPremultiplied)
                }
                _ => {
                    let animation = (!layer.text_animators.is_empty()).then_some((
                        layer.text_animators.as_slice(),
                        frame,
                        self.time.fraction(),
                    ));
                    let (texture, local_origin) =
                        if animation.is_some() || matches!(content, std::borrow::Cow::Owned(_)) {
                            self.sampled_texture(&content, animation)?
                        } else {
                            self.raster_texture(&content)?
                        };
                    (
                        texture,
                        translated(origin, local_origin),
                        TexelColor::SrgbPremultiplied,
                    )
                }
            };
            let mut wipe_right = None;
            if let Some(range) = layer.wipe {
                let width = self.time.reveal(range, source.width());
                if width == 0 {
                    continue;
                }
                if width < source.width() {
                    let left = if matches!(&layer.content, Content::Group { .. }) {
                        origin[0]
                    } else {
                        artwork_origin[0]
                    };
                    wipe_right = Some(left + i64::from(width));
                }
            }
            let mask = if let Some(mask) = &layer.mask {
                let (tex, crop) = self.mask_texture(mask, frame)?;
                Some((tex, translated(translated(origin, mask.position), crop)))
            } else {
                None
            };
            // A shadow can enter the stage from a cropped-out source. Otherwise
            // an invisible source never needs its spatial effect passes.
            if !layer.effects.is_empty()
                && layer.underlay.is_none()
                && !matches!(content.as_ref(), Content::Group { .. })
                && Bounds::texture(&source, artwork_origin)
                    .clip_source(
                        mask.as_ref().map(|(tex, pos)| Bounds::texture(tex, *pos)),
                        wipe_right,
                    )
                    .transformed(origin, transform)
                    .intersect(Bounds::texture(target, [0; 2]))
                    .empty()
            {
                continue;
            }
            let mut prepared = PreparedSource::new(source, color);
            self.source_effects(
                &mut prepared,
                layer,
                &content,
                frame,
                [artwork_origin[0] - origin[0], artwork_origin[1] - origin[1]],
            )?;
            let features = prepared.features();
            let (body, amount) = match &layer.filter {
                Some(effect) => (
                    self.cache.shader(&effect.shader)?,
                    f32::from(effect.amount) / 100.,
                ),
                None => (IDENTITY_EFFECT, 0.),
            };
            if layer.filter.is_some() || !layer.effects.is_empty() {
                self.stats.filtered_layers += 1;
            }
            let pipeline = self
                .gpu
                .point_pipeline(layer.blend, body, features, mattes)?;
            // An isolated image with no authored-coordinate paint inputs is
            // already in the destination pixel grid after its children draw.
            let isolated = matches!(&layer.content, Content::Group { .. });
            let transform =
                if isolated && mask.is_none() && wipe_right.is_none() && layer.filter.is_none() {
                    None
                } else {
                    transform
                };
            let draw = Draw {
                blend: layer.blend,
                destination_art: isolated && transform.is_some(),
                transform,
                origin,
                artwork_origin,
                mask: mask.as_ref().map(|(texture, pos)| (texture, *pos)),
                wipe_right,
                opacity,
                amount,
                color: prepared.color,
                blur: prepared.blur_input(),
                grades: &prepared.grades,
                object_mattes: object_mattes.as_ref(),
                pipeline: &pipeline,
            };
            if let Some(underlay) = &layer.underlay {
                // Capture the filtered caster on transparency before painting
                // its shadow. The final blend reads the destination *after* the shadow.
                let caster = if layer.blend == Blend::Multiply {
                    self.gpu
                        .point_pipeline(Blend::SourceOver, body, features, mattes)?
                } else {
                    pipeline.clone()
                };
                self.shadow(
                    target,
                    &prepared.source,
                    Draw {
                        blend: Blend::SourceOver,
                        pipeline: &caster,
                        ..draw
                    },
                    underlay,
                )?;
            }
            let pixels = self.gpu.paint(
                self.commands,
                target,
                &prepared.source,
                draw,
                wgpu::LoadOp::Load,
            )?;
            if layer.blend == Blend::Multiply {
                self.stats.multiply_pixels += pixels;
                self.stats.multiply_copy_bytes += pixels * 4;
            }
        }
        Ok(())
    }
}

pub(crate) fn raster_bounds(content: &Content, cache: &PaintCache) -> Result<([i32; 2], [u32; 2])> {
    if let Some(bounds) = cache.cached_raster_bounds(content) {
        return Ok(bounds);
    }
    match content {
        Content::Path(native) => super::path::raster_bounds(native),
        Content::Rect(rect) => rect.raster_bounds(),
        Content::Svg { source } => Ok(([0; 2], super::svg::raster_size(source)?)),
        Content::Image { source } => Ok(([0; 2], crate::render::image::dimensions(source)?)),
        Content::Label { .. } => cache.probe_label_bounds(content),
        _ => anyhow::bail!("expected raster artwork"),
    }
}

fn translated(parent: [i64; 2], local: [i32; 2]) -> [i64; 2] {
    [
        parent[0] + i64::from(local[0]),
        parent[1] + i64::from(local[1]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::affine::SampledPose;

    #[test]
    fn effect_bounds_include_independent_scale_and_translation_between_keys() -> Result<()> {
        let layer: Graphic = serde_json::from_value(serde_json::json!({
            "content": {"type": "transform_group", "children": []},
            "channels": {
                "x": [{"frame":0,"value":0},{"frame":10,"value":100}],
                "scale": [{"frame":0,"value":1,"ease":"hold"},{"frame":10,"value":0.1}]
            }
        }))?;
        // The caster starts at x=100 and ends at x=110. In between it reaches
        // x=150 while scale still holds. Comparing the two keyed poses misses it.
        let caster = Bounds::new([100, 0], [2, 4]);
        let envelope = caster.motion_envelope([0; 2], &layer);
        let mid = caster.transformed([0; 2], SampledTransform::new(None, layer.motion_at(5, 0.)));
        assert!(mid.left >= 149 && mid.right >= 152);
        assert!(envelope.intersect(mid) == mid);
        Ok(())
    }

    #[test]
    fn an_extreme_intermediate_envelope_survives_until_destination_clipping() {
        let pose = |x| {
            SampledTransform::new(
                None,
                SampledPose {
                    x,
                    ..Default::default()
                },
            )
        };
        let surface = Bounds::new([0; 2], [64; 2]);
        let huge = surface.transformed([0; 2], pose(1e25));
        assert!(huge == Bounds::UNBOUNDED);
        let restored = huge.transformed([0; 2], pose(-1e25));
        assert!(restored.intersect(surface) == surface);
    }

    #[test]
    fn odd_width_readback_preserves_each_row_and_composites_in_linear_light() -> Result<()> {
        let gpu = Gpu::new()?;
        let (width, height) = (641, 3);
        let mut plate = [0, 0, 0, 255].repeat(width * height);
        for row in 0..height {
            let last = ((row + 1) * width - 1) * 4;
            plate[last..last + 4].copy_from_slice(&[row as u8, 100, 201, 255]);
        }
        assert_eq!(
            gpu.compose(width as u32, height as u32, &plate, None)?,
            plate
        );
        // The red pixel falls outside the left edge. Only the half-covered white
        // pixel is visible, on black, at the beginning of the second row.
        let artwork = [255, 0, 0, 255, 128, 128, 128, 128];
        let frame = gpu.compose(
            width as u32,
            height as u32,
            &plate,
            Some((&artwork, 2, 1, -1, 1, 1.0)),
        )?;
        let changed = width * 4;
        assert!((187..=189).contains(&frame[changed]));
        assert_eq!(
            &frame[changed..changed + 4],
            &[frame[changed], frame[changed], frame[changed], 255]
        );
        assert_eq!(&frame[..changed], &plate[..changed]);
        assert_eq!(&frame[changed + 4..], &plate[changed + 4..]);
        Ok(())
    }
    #[test]
    fn a_backmost_plate_keeps_a_pristine_source_for_a_later_nested_footage_read() -> Result<()> {
        use serde_json::json;
        use std::path::Path;
        let gpu = Gpu::new()?;
        let video = [10, 20, 30, 255].repeat(12 * 8);
        let mut graphics: Vec<Graphic> = serde_json::from_value(json!([
            {"content":{"type":"footage"}},
            {"content":{"type":"path","d":"M0 0H12V8H0Z","fill":[255,0,0,255]}, "wipe":[0,2]},
            {"content":{"type":"group","children":[
                {"content":{"type":"footage"},"mask":{"content":{"type":"path","d":"M0 0H2V8H0Z"}}}
            ]}}
        ]))?;
        let exposure = Some(Exposure {
            angle_degrees: 360,
            samples: 4,
        });
        let draw = |layers: &[Graphic]| -> Result<Vec<u8>> {
            let inputs = FrameInputs {
                size: [12, 8],
                primary: PrimaryPicture::Video(&video),
                video_layers: &HashMap::new(),
                overlay: None,
                scopes: vec![(layers, 0)],
            };
            Ok(gpu
                .compose_exposed(inputs, exposure, &mut PaintCache::default())?
                .0)
        };
        let accelerated = draw(&graphics)?;
        // A zero-strength point filter on the first layer exercises ordinary
        // source-over compositing, which is the reference for the root fast path.
        graphics[0].filter = Some(graphics::Filter {
            shader: Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/dry-ink.wgsl"),
            amount: 0,
            mattes: vec![],
        });
        assert_eq!(accelerated, draw(&graphics)?);
        assert_eq!(accelerated[..8], video[..8]);
        assert!(accelerated[8] > 30 && accelerated[8] < 255);
        Ok(())
    }

    #[test]
    fn a_custom_effect_can_target_a_path_svg_video_or_isolated_group() -> Result<()> {
        use crate::graphics::Filter;
        use std::{fs, path::Path};
        let gpu = Gpu::new()?;
        let folder = tempfile::tempdir()?;
        let svg = folder.path().join("square.svg");
        fs::write(
            &svg,
            r##"<svg width="8" height="8"><path d="M0 0 H8 V8 H0 Z" fill="#ff0000"/></svg>"##,
        )?;
        let result = |content| Graphic {
            name: None,
            content,
            blend: Blend::SourceOver,
            motion: vec![],
            channels: None,
            transform: None,
            span: None,
            type_on: None,
            text_anchor: None,
            text_animators: vec![],
            position: [0, 0],
            wipe: None,
            filter: None,
            underlay: None,
            effects: vec![],
            mask: None,
        };
        let mut red = result(Content::Svg { source: svg });
        red.position = [4, 4];
        let effect = Filter {
            shader: Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/dry-ink.wgsl"),
            amount: 100,
            mattes: vec![],
        };
        let plate = [120, 120, 120, 255].repeat(16 * 16);
        let mut cache = PaintCache::default();
        let mut run =
            |layers: &[Graphic]| gpu.compose_scene([16, 16], &plate, None, layers, 0, &mut cache);
        let (clean, _) = run(&[red.clone()])?;
        red.filter = Some(effect.clone());
        let (ink, stats) = run(&[red.clone()])?;
        assert_eq!(stats.graphic_uploads, 0);
        assert_eq!(stats.filtered_layers, 1);
        assert_ne!(clean, ink);
        for (p, (a, b)) in clean
            .as_chunks::<4>()
            .0
            .iter()
            .zip(ink.as_chunks::<4>().0)
            .enumerate()
        {
            if !(4..12).contains(&(p % 16)) || !(4..12).contains(&(p / 16)) {
                assert_eq!(a, b);
            }
        }
        // The same WGSL must see authored coordinates even when Skia tightly
        // crops the native path instead of painting into a fixed SVG viewport.
        let mut path = result(Content::Path(graphics::NativePath {
            d: "M0 0 H8 V8 H0 Z".into(),
            fill_rule: Default::default(),
            fill: Some([255, 0, 0, 255]),
            stroke_start: None,
            stroke_end: None,
            stroke: None,
        }));
        path.position = red.position;
        path.filter = red.filter.clone();
        assert_eq!(run(&[path.clone()])?.0, ink);
        // Extend only the local bounds. Ink on the original square stays put.
        if let Content::Path(ref mut native) = path.content
            && let graphics::PathData::Svg(d) = &mut native.d
        {
            d.push_str(" M-3 -3 h1 v1 h-1 Z");
        }
        let expanded = run(&[path.clone()])?.0;
        for y in 4..12 {
            let row = (y * 16 + 4) * 4..(y * 16 + 12) * 4;
            assert_eq!(expanded[row.clone()], ink[row]);
        }
        path.position[0] += 1;
        let (translated, stats) = run(&[path.clone()])?;
        assert_eq!((stats.graphic_uploads, stats.raster_paints), (0, 0));
        for y in 4..12 {
            let from = (y * 16 + 4) * 4..(y * 16 + 12) * 4;
            assert_eq!(translated[from.start + 4..from.end + 4], ink[from]);
        }
        path.filter.as_mut().unwrap().amount = 20;
        let (lighter, stats) = run(&[path])?;
        assert_eq!((stats.graphic_uploads, stats.raster_paints), (0, 0));
        assert_ne!(translated, lighter);
        let mut video = result(Content::Footage { history: None });
        video.filter = Some(effect.clone());
        let (filtered_plate, _) = run(&[video.clone()])?;
        assert_ne!(filtered_plate, plate);
        assert!(
            filtered_plate
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 255)
        );
        red.filter = None;
        video.filter = None;
        let mut group = result(Content::Group {
            children: vec![video, red],
        });
        group.filter = Some(effect);
        let (filtered_group, _) = run(&[group])?;
        assert_ne!(filtered_group, filtered_plate);
        assert_ne!(filtered_group, ink);
        for row in [0, 1, 15] {
            let range = row * 16 * 4..(row + 1) * 16 * 4;
            assert_eq!(&filtered_group[range.clone()], &filtered_plate[range]);
        }
        Ok(())
    }

    #[test]
    fn rectangle_mask_and_drawable_share_local_sample_coverage_and_held_texture() -> Result<()> {
        use serde_json::json;
        let gpu = Gpu::new()?;
        let size = [32, 24];
        let rect = json!({"type":"rect","x":[{"frame":0,"value":2},{"frame":1,"value":4}],
            "y":3,"width":8,"height":10,
            "corner_radius":[{"frame":0,"value":1.5},{"frame":1,"value":[1.5,2,3,0]}]});
        let over = |children| {
            json!({"position":[1,2], "span":[3,10],
            "content":{"type":"transform_group","children":children}})
        };
        let mut white = rect.clone();
        white["fill"] = json!([255, 255, 255, 255]);
        let direct: Graphic = serde_json::from_value(over(json!([
            {"content":white, "span":[2,7]}
        ])))?;
        let children = json!([
            {"content":{"type":"rect","width":26,"height":22,"fill":[0,255,0,255]}},
            {"content":{"type":"rect","width":26,"height":22,"fill":[255,255,255,255]}}
        ]);
        let clipped: Graphic = serde_json::from_value(over(json!([
            {"span":[2,7], "content":{"type":"group","children":children}, "mask":{"content":rect}}
        ])))?;
        let black = [0, 0, 0, 255].repeat(32 * 24);
        let mut cache = PaintCache::default();
        let mut run = |layer: &Graphic, frame, exposure| {
            gpu.compose_exposed(
                FrameInputs {
                    size,
                    primary: PrimaryPicture::Video(&black),
                    video_layers: &HashMap::new(),
                    overlay: None,
                    scopes: vec![(std::slice::from_ref(layer), frame)],
                },
                exposure,
                &mut cache,
            )
        };
        let shutter = Some(Exposure {
            samples: 2,
            angle_degrees: 360,
        });
        let (_, cold) = run(&clipped, 5, shutter)?;
        assert!(cold.graphic_uploads >= 2);
        let (soft, warm) = run(&clipped, 5, shutter)?;
        assert_eq!(warm.graphic_uploads, 0);
        assert_eq!(soft, run(&direct, 5, shutter)?.0);
        let parked = run(&direct, 5, None)?.0;
        assert_ne!(parked, soft);
        // Fractional mask positions 2.5 and 3.5 on a held composition.
        // Exposure-averaged linear coverage at the first edge is about 25%.
        let edge = (8 * 32 + 3) * 4;
        assert!((134..=139).contains(&soft[edge]));
        assert_eq!(soft[edge], soft[edge + 1]); // green underneath cannot leak
        assert_eq!(&soft[edge - 4..edge], &[0, 0, 0, 255]);
        let at_cut = run(&clipped, 6, None)?.0;
        let (held, stats) = run(&clipped, 8, None)?;
        assert_eq!(at_cut, held);
        assert_eq!((stats.raster_paints, stats.graphic_uploads), (0, 0));
        assert_eq!(run(&clipped, 4, None)?.0, black);
        let mut rotated = direct.clone();
        rotated.transform = Some(graphics::Transform {
            pivot: [8., 8.],
            rotation_degrees: Some(90.),
            ..Default::default()
        });
        let mut rotated_mask = clipped;
        rotated_mask.transform = rotated.transform;
        assert_eq!(
            run(&rotated, 5, shutter)?.0,
            run(&rotated_mask, 5, shutter)?.0
        );
        Ok(())
    }

    #[test]
    fn path_and_svg_masks_apply_once_in_owner_coordinates() -> Result<()> {
        use graphics::{Filter, Mask, MaskContent};
        let gpu = Gpu::new()?;
        let folder = tempfile::tempdir()?;
        let svg = folder.path().join("silhouette.svg");
        let contour = "M0.5 0.5 H8.5 V6.5 H0.5 Z";
        std::fs::write(
            &svg,
            format!(r##"<svg width="10" height="8"><path d="{contour}" fill="#00ff00"/></svg>"##),
        )?;
        let path = |d: &str, color| Graphic {
            name: None,
            blend: Blend::SourceOver,
            motion: vec![],
            channels: None,
            transform: None,
            span: None,
            type_on: None,
            text_anchor: None,
            text_animators: vec![],
            content: Content::Path(graphics::NativePath {
                d: d.into(),
                fill_rule: Default::default(),
                fill: Some(color),
                stroke_start: None,
                stroke_end: None,
                stroke: None,
            }),
            position: [0; 2],
            filter: None,
            underlay: None,
            effects: vec![],
            wipe: None,
            mask: None,
        };
        let green = path("M-4 -4 H9 V9 H-4 Z", [0, 255, 0, 255]);
        let red = path("M-4 -4 H9 V9 H-4 Z", [255, 0, 0, 255]);
        let mut group = Graphic {
            name: None,
            blend: Blend::SourceOver,
            motion: vec![],
            channels: None,
            transform: None,
            span: None,
            type_on: None,
            text_anchor: None,
            text_animators: vec![],
            content: Content::Group {
                children: vec![green, red],
            },
            position: [6, 4],
            filter: None,
            underlay: None,
            effects: vec![],
            wipe: None,
            mask: Some(Mask {
                shader: None,
                position: [-3, -2],
                content: MaskContent::Path {
                    d: contour.into(),
                    fill_rule: Default::default(),
                },
            }),
        };
        let mut cache = PaintCache::default();
        let mut plate = [0, 0, 0, 255].repeat(20 * 15);
        plate[6 * 20 * 4..].fill(255);
        let mut run = |layer: &Graphic| {
            gpu.compose_scene(
                [20, 15],
                &plate,
                None,
                std::slice::from_ref(layer),
                0,
                &mut cache,
            )
        };
        let mut expected = path(contour, [255, 0, 0, 255]);
        expected.position = [3, 2];
        let reference = run(&expected)?.0;
        let pixels = run(&group)?.0;
        assert_eq!(reference, pixels);
        // A mask multiplied into each child separately would expose green
        // beneath the half-covered red edge. Authored content also extends
        // left of the group's origin (x=6).
        let edge = (20 * 5 + 3) * 4;
        assert!((175..=200).contains(&pixels[edge]));
        assert_eq!(&pixels[edge + 1..edge + 4], &[0, 0, 255]);
        assert_eq!(&pixels[edge + 4..edge + 8], &[255, 0, 0, 255]);
        // The same half-covered red edge on white retains full red. The two
        // other channels carry half the plate's linear intensity (~188 sRGB).
        let bright_edge = (20 * 6 + 3) * 4;
        assert_eq!(pixels[bright_edge], 255);
        assert!((186..=190).contains(&pixels[bright_edge + 1]));
        assert_eq!(pixels[bright_edge + 1], pixels[bright_edge + 2]);
        assert_eq!(pixels[bright_edge + 3], 255);
        assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
        // A filter can replace its input alpha. The layer's mask still
        // controls the final filtered edge, including fractional coverage.
        let shader = folder.path().join("solid-red.wgsl");
        std::fs::write(
            &shader,
            "fn effect(p: vec2<i32>, c: vec4<f32>, a: f32) -> vec4<f32> { return mix(c, vec4(1., 0., 0., 1.), a); }",
        )?;
        let mut leaf = path("M-4 -4 H9 V9 H-4 Z", [0, 255, 0, 128]);
        leaf.position = group.position;
        leaf.mask = group.mask.clone();
        leaf.filter = Some(Filter {
            shader,
            amount: 100,
            mattes: vec![],
        });
        assert_eq!(run(&leaf)?.0, reference);
        group.mask.as_mut().unwrap().content = MaskContent::Svg { source: svg };
        // An SVG is painted on the CPU and a path on the device. On one painter the two masks
        // give the same picture. On the default pair only fractional corners differ.
        let cpu = Gpu::reference()?;
        let mut cpu_cache = PaintCache::default();
        let mut on_cpu = |layer: &Graphic| {
            cpu.compose_scene(
                [20, 15],
                &plate,
                None,
                std::slice::from_ref(layer),
                0,
                &mut cpu_cache,
            )
        };
        assert_eq!(on_cpu(&group)?.0, on_cpu(&expected)?.0);
        let corners =
            super::super::skia_gpu::bytes_beyond_edge_rounding(&run(&group)?.0, &reference);
        // Two corner pixels, three colour channels each.
        assert!(corners <= 6, "{corners} bytes differ beyond edge rounding");

        group.position = [2, 1];
        expected.position = [-1, -1];
        let clipped = run(&expected)?.0;
        let (moved_group, stats) = run(&group)?;
        // The group still carries the SVG mask: exact on one painter, as above.
        assert_eq!(on_cpu(&group)?.0, on_cpu(&expected)?.0);
        assert!(super::super::skia_gpu::bytes_beyond_edge_rounding(&clipped, &moved_group) <= 6);
        assert_eq!(stats.graphic_uploads, 0);

        // Fractional input alpha throughout the interior, independent of edge AA.
        let wash = folder.path().join("wash.svg");
        std::fs::write(
            &wash,
            r##"<svg width="10" height="8"><g opacity="0.5"><path d="M0 0H10V8H0Z" fill="#ffffff"/></g></svg>"##,
        )?;
        group.mask.as_mut().unwrap().content = MaskContent::Svg { source: wash };
        let (washed, _) = run(&group)?;
        let inside = (20 * 3 + 4) * 4;
        assert!((186..=190).contains(&washed[inside]));
        assert_eq!(&washed[inside + 1..inside + 4], &[0, 0, 255]);
        Ok(())
    }
}
