//! Native frame evaluation shared by the CLI, offline encoder and presentation probe.
pub mod camera;
#[cfg(feature = "host-audio")]
pub mod editor;
pub mod export;
pub mod graphics;
#[cfg(feature = "host-audio")]
pub mod host;
pub mod inspect;
mod media;
mod plan;
pub mod preview;
mod project;
pub mod project_watch;
pub mod proxy;
mod render;
pub mod session;
pub mod transport;

#[cfg(test)]
mod backdrop_tests;
#[cfg(test)]
mod shadow_tests;

use anyhow::{Context, Result};
use media::{IndexedVideo, assets::SourceProfiles};
use render::{gpu::Gpu, text};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use plan::{CameraSelection, FrameRequest, OverlayFrameRequest, VideoFrameRequest};
pub use project::{
    AuthoringDocument, Background, Backplate, FrameRate, GraphicsPlacement, OccurrenceRanges,
    OutputDamage, Project, ProjectDamage, RevisionChange, Shot, TimelineChange, Title,
};
pub use render::path_trace::FinalCancel;

pub(crate) const MAX_SURFACE_DIMENSION: u32 = 8192;

pub struct Engine {
    project: Project,
    revision: u64,
    gpu: Gpu,
    sources: SourceProfiles,
    reader: media::VideoReader,
    paragraph: Option<text::Paragraph>,
    graphics: graphics::PaintCache,
    path_tracer: render::path_trace::PathTracer,
}

#[derive(Clone, Debug, Serialize)]
pub struct RenderTrace {
    pub revision: u64,
    pub request: FrameRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<PathBuf>,
    pub paragraph_lines: Option<usize>,
    /// Backplate [left, top, width, height] in output pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backplate_rect: Option<[i64; 4]>,
    pub reused_decode: bool,
    pub reused_paragraph: bool,
    /// This presentation came from the interactive composed-picture cache.
    pub reused_composite: bool,
    pub graphics: graphics::PaintStats,
}

#[derive(Clone)]
pub struct Frame {
    pub size: [u32; 2],
    pub rgba: Arc<Vec<u8>>,
    pub trace: RenderTrace,
}

impl Engine {
    pub fn new(project: Project) -> Result<Self> {
        Self::new_with_artwork(project, None, None)
    }

    fn new_with_artwork(
        mut project: Project,
        raster_assets: Option<&graphics::PreparedRasterAssets>,
        text_layouts: Option<&graphics::PreparedTextLayouts>,
    ) -> Result<Self> {
        project.validate()?;
        let mut graphics = graphics::PaintCache::default();
        if let Some(assets) = raster_assets {
            graphics.install_raster_assets(assets);
        } else {
            graphics
                .install_raster_assets(&graphics::PreparedRasterAssets::prepare_pinned(&project)?);
        }
        if let Some(layouts) = text_layouts {
            graphics.install_text_layouts(layouts);
        } else {
            graphics.place_text_anchors(&mut project, None)?;
        }
        graphics.prepare_spatial_meshes(&project)?;
        Gpu::validate_backdrop_scratch(&project, &graphics)?;
        Gpu::validate_shadow_scratch(&project, &graphics)?;
        Gpu::validate_source_blur_scratch(&project, &graphics)?;
        let sources = graphics
            .animations
            .profiles(&project, &HashMap::new(), true)?;
        let gpu = Gpu::new()?;
        for layers in project.graphic_scopes() {
            gpu.validate_effects(layers, &mut graphics, project.size)?;
        }
        Ok(Self {
            project,
            revision: 0,
            gpu,
            sources,
            reader: Default::default(),
            paragraph: None,
            graphics,
            path_tracer: Default::default(),
        })
    }

    pub fn adapter_name(&self) -> &str {
        self.gpu.adapter_name()
    }

    /// A read-only device allocation counter for an external performance sampler.
    /// Returns None on backends that do not expose resident allocation sizes.
    pub fn gpu_allocation_probe(&self) -> impl Fn() -> Option<u64> + Send + 'static {
        self.gpu.allocation_probe()
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Pin remaining external artwork when opening a live edit session,
    /// including SVGs referenced only as masks. All hosts prepare USD meshes at open.
    fn prepare_session_artwork(&mut self) -> Result<()> {
        let staged = self.graphics.stage_rasters(&self.project, None)?;
        self.paragraph = text::paint_project_title(&self.project.title)?;
        self.graphics.install_rasters(staged);
        Ok(())
    }

    /// Prepare changed source indices before installing an edit. Unchanged decoders
    /// and shaped paragraphs can survive an edit at a parked frame.
    pub fn replace_project(&mut self, mut next: Project) -> Result<()> {
        next.validate()?;
        if self.project == next {
            return Ok(());
        }
        let mut layout = graphics::PaintCache::default();
        layout.place_text_anchors(&mut next, Some(&self.graphics))?;
        let same_profile = next.size == self.project.size && next.fps == self.project.fps;
        let graphics_changed = next.graphic_scopes().ne(self.project.graphic_scopes());
        // Longer cuts and changed exposure can reveal more of a wiped mask.
        Gpu::validate_backdrop_scratch(&next, &self.graphics)?;
        Gpu::validate_shadow_scratch(&next, &self.graphics)?;
        Gpu::validate_source_blur_scratch(&next, &self.graphics)?;
        let staged = if graphics_changed {
            Some(
                self.graphics
                    .stage_with_layout(&next, Some(&self.project), layout)?,
            )
        } else {
            None
        };
        let source_changes =
            if !same_profile || self.project.shots != next.shots || graphics_changed {
                let animations = media::assets::PreparedAnimations::prepare(&next, |_| {
                    Some(&self.graphics.animations)
                })?;
                let empty = SourceProfiles::new();
                let existing = if same_profile { &self.sources } else { &empty };
                let profiles = animations.profiles(&next, existing, true)?;
                Some((animations, profiles))
            } else {
                None
            };
        if (graphics_changed || !same_profile)
            && let Err(error) = next.graphic_scopes().try_for_each(|scope| {
                self.gpu
                    .validate_effects(scope, &mut self.graphics, next.size)
            })
        {
            // A failed compile may have read a new shader path. On retry,
            // read its corrected source while keeping the accepted shaders pinned.
            self.graphics.retain(&self.project);
            return Err(error);
        }
        if let Some((animations, new)) = source_changes {
            self.graphics.animations = animations;
            // Artwork on a material can sample footage. Its authored layers can be
            // identical while its decoded frame changes with the output/source profile.
            if !same_profile
                || new.len() != self.sources.len()
                || new.iter().any(|(key, source)| {
                    self.sources
                        .get(key)
                        .is_none_or(|previous| !Arc::ptr_eq(source, previous))
                })
            {
                self.graphics.live_surfaces.clear();
            }
            if same_profile {
                self.reader
                    .retain_sources(|key| new.values().any(|index| index.has_identity(key)));
            } else {
                self.reader = Default::default();
            }
            self.sources = new;
        }
        let old = &self.project.title;
        let title = &next.title;
        if old.text != title.text
            || old.font != title.font
            || old.font_size != title.font_size
            || old.width != title.width
            || old.backplate != title.backplate
        {
            self.paragraph = None;
        }
        if let Some(staged) = staged {
            self.graphics.retain(&next);
            self.graphics.install_rasters(staged);
        }
        self.project = next;
        self.revision += 1;
        Ok(())
    }

    /// Apply both title changes as one revision. An invalid edit preserves the previous state.
    pub fn edit_title(&mut self, start: Option<u32>, width: Option<u32>) -> Result<()> {
        let mut next = self.project.clone();
        let title = &mut next.title;
        title.start = start.unwrap_or(title.start);
        title.width = width.unwrap_or(title.width);
        self.replace_project(next)
    }

    /// Render a picture now. A path-traced scene is shown from its finished Final when one
    /// exists, and otherwise as a rasterized Draft; `trace.graphics.path_traced_draft` counts
    /// those, so a presenter can label the picture.
    pub fn render(&mut self, frame: u32) -> Result<Frame> {
        self.graphics.finals.take_pending();
        self.render_view(frame, None)
    }

    /// Why this build cannot finish a path-traced scene, or `None` when it can.
    pub fn path_trace_unavailable() -> Option<&'static str> {
        render::path_trace::unavailable()
    }

    /// Keep finished path-traced pictures in a directory, so they outlive this engine.
    /// Nothing is ever deleted from a directory named this way.
    pub fn persist_finals(&mut self, directory: PathBuf) -> Result<()> {
        self.graphics.finals.persist_in(directory, None)
    }

    /// Keep them in the user's cache, shared by every project and host on this machine. Keys
    /// describe content, so a copy of a project finds the same pictures. `N4_FINAL_STORE` names
    /// another directory. The cache holds 20 GB, or `N4_FINAL_STORE_GB`; beyond that the
    /// pictures used longest ago are deleted and would be traced again.
    pub fn persist_finals_in_user_cache(&mut self) -> Result<()> {
        let directory = match std::env::var_os("N4_FINAL_STORE") {
            Some(directory) => PathBuf::from(directory),
            None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
                .join("Library/Caches/n4/final"),
        };
        let gigabytes = match std::env::var("N4_FINAL_STORE_GB") {
            Ok(value) => value
                .parse::<u64>()
                .ok()
                .filter(|&g| g > 0)
                .context("N4_FINAL_STORE_GB is a whole number of gigabytes above zero")?,
            Err(_) => 20,
        };
        self.graphics
            .finals
            .persist_in(directory, Some(gigabytes << 30))
    }

    /// A handle that interrupts `render_final` from another thread.
    pub fn final_cancel(&self) -> FinalCancel {
        self.path_tracer.canceller()
    }

    /// Render a picture whose path-traced scenes are all Final, tracing any that are missing
    /// with the recipe authored on the scene. The result does not depend on what was rendered
    /// before. Returns the picture and the number of scenes traced by this call.
    pub fn render_final(&mut self, frame: u32) -> Result<(Frame, u32)> {
        let mut traced = 0;
        // A traced scene can sit behind another one only a few levels deep.
        for _ in 0..4 {
            let picture = self.render(frame)?;
            let pending = self.graphics.finals.take_pending();
            if pending.is_empty() {
                return Ok((picture, traced));
            }
            for request in pending {
                let mut artwork = Vec::with_capacity(request.models.len());
                for model in &request.models {
                    let mut pixels = Vec::with_capacity(model.surfaces.len());
                    for surface in &model.surfaces {
                        pixels.push(self.gpu.read_texture(&surface.texture)?);
                    }
                    artwork.push(pixels);
                }
                let finished = self
                    .path_tracer
                    .render(&request, &self.graphics.meshes, artwork)
                    .context("path-traced Final")?;
                self.graphics.finals.insert(request.key, finished)?;
                traced += 1;
            }
        }
        anyhow::bail!("path-traced scenes did not settle at frame {frame}")
    }

    /// Reconstruct trace metadata for a picture whose pixels survived a revision.
    fn resolved_request(&self, frame: u32) -> Result<FrameRequest> {
        let mut request = self.project.request(frame)?;
        resolve_request_sources(&self.sources, self.project.size, &mut request)?;
        Ok(request)
    }

    /// An isolated thumbnail can select a subset of the accepted project's
    /// scopes while sharing its prepared graphics and decode profiles.
    fn render_view(&mut self, frame: u32, view: Option<&Project>) -> Result<Frame> {
        let project = view.unwrap_or(&self.project);
        let mut request = project.request(frame)?;
        let (shot, shot_frame) = project.shot_at(frame)?;
        let mut history = Vec::with_capacity(request.source_history.len());
        let sources = resolve_request_sources(&self.sources, project.size, &mut request)?;
        let primary = sources.primary;
        self.reader.begin_frame(
            primary
                .into_iter()
                .chain(sources.layers.iter().map(|(source, _)| *source)),
        );
        let (video, reused_decode) = if let Some(source) = primary {
            // Read the window in presentation order so a cold seek can decode forward once.
            for entry in request.source_history.iter().rev() {
                let prior = if let Some(tick) = entry {
                    Some(self.reader.read(source, *tick)?.0.rgba)
                } else {
                    None
                };
                history.push(prior);
            }
            history.reverse();
            let ordinal = request
                .source_frame
                .context("missing primary source ordinal")?;
            let (pixels, reused) = self.reader.read(source, ordinal)?;
            (Some(pixels), reused)
        } else {
            (None, false)
        };
        let [width, height] = project.size;
        // Preserve the authored tick for painter lookup, and expose the actual
        // presentation ordinal in the trace (a VFR hold may map several ticks to one).
        let mut video_layers = HashMap::new();
        for (layer, (source, tick)) in request.video_layers.iter().zip(sources.layers) {
            let key = (layer.source.as_path(), layer.size, tick);
            if let std::collections::hash_map::Entry::Vacant(entry) = video_layers.entry(key) {
                entry.insert(self.reader.read(source, layer.source_frame)?.0);
            }
        }
        let [num, den] = request.title_opacity;
        let title_visible = num > 0 && (!request.text.is_empty() || request.backplate.is_some());
        let reused_paragraph = title_visible && self.paragraph.is_some();
        if title_visible && self.paragraph.is_none() {
            self.paragraph = text::paint_project_title(&project.title)?;
        }
        let visible = self.paragraph.as_ref().filter(|_| title_visible);
        let padding = request
            .backplate
            .map_or([0, 0], |b| b.padding.map(i32::from));
        let [x, y] = [
            request.title_position[0].saturating_sub(padding[0]),
            request.title_position[1].saturating_sub(padding[1]),
        ];
        let backplate_rect = visible
            .and_then(|p| p.plate_size)
            .map(|[w, h]| [i64::from(x), i64::from(y), i64::from(w), i64::from(h)]);
        let overlay = visible.map(|p| {
            (
                p.rgba.as_slice(),
                p.width,
                p.height,
                x,
                y,
                num as f32 / den as f32,
            )
        });
        let inputs = render::gpu::FrameInputs {
            size: project.size,
            primary: match (&video, &shot.background) {
                (Some(rgba), None) if !history.is_empty() => {
                    render::gpu::PrimaryPicture::DecodedHistory(rgba, &history)
                }
                (Some(rgba), None) => render::gpu::PrimaryPicture::Decoded(rgba),
                (None, Some(background)) => render::gpu::PrimaryPicture::Background(background),
                _ => anyhow::bail!("expected exactly one primary picture"),
            },
            video_layers: &video_layers,
            overlay,
            scopes: project.paint_scopes(shot, shot_frame, frame).collect(),
        };
        let (rgba, graphics) = self.gpu.compose_view(
            inputs,
            project.exposure,
            shot.camera_at(shot_frame),
            &mut self.graphics,
        )?;
        Ok(Frame {
            size: [width, height],
            rgba: Arc::new(rgba),
            trace: RenderTrace {
                revision: self.revision,
                request,
                proxy: None,
                paragraph_lines: visible.map(|p| p.lines),
                backplate_rect,
                reused_decode,
                reused_paragraph,
                reused_composite: false,
                graphics,
            },
        })
    }
}

struct RequestSources<'a> {
    primary: Option<&'a IndexedVideo>,
    /// Each layer's index and authored tick, in paint order.
    layers: Vec<(&'a IndexedVideo, u32)>,
}

/// Resolve authored ticks to presentation ordinals without decoding any pictures.
fn resolve_request_sources<'a>(
    sources: &'a SourceProfiles,
    size: [u32; 2],
    request: &mut FrameRequest,
) -> Result<RequestSources<'a>> {
    let primary = request
        .source
        .as_ref()
        .map(|path| {
            sources
                .get(&(path.clone(), size))
                .map(Arc::as_ref)
                .context("missing validated source")
        })
        .transpose()?;
    if let Some(source) = primary {
        for tick in request.source_history.iter_mut().flatten() {
            *tick = source.resolve(*tick)?;
        }
        let tick = request
            .source_frame
            .context("missing primary source tick")?;
        request.source_frame = Some(source.resolve(tick)?);
    }
    let layers = request
        .video_layers
        .iter_mut()
        .map(|layer| {
            let source = sources
                .get(&(layer.source.clone(), layer.size))
                .context("missing validated video layer source")?;
            let tick = layer.source_frame;
            layer.source_frame = source.resolve(tick)?;
            Ok((source.as_ref(), tick))
        })
        .collect::<Result<_>>()?;
    Ok(RequestSources { primary, layers })
}

/// Fallibly allocate a zeroed buffer with a bulk clear, also in debug builds.
pub(crate) fn zeroed_bytes(len: usize) -> Result<Vec<u8>, std::collections::TryReserveError> {
    let mut pixels = Vec::<u8>::new();
    pixels.try_reserve_exact(len)?;
    // SAFETY: the reservation provides len writable bytes; zero initializes every u8
    // before the vector's length is published or any caller can read the surface.
    unsafe {
        pixels.as_mut_ptr().write_bytes(0, len);
        pixels.set_len(len);
    }
    Ok(pixels)
}
