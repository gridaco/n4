//! Provisional fixture input. Its file representation stops at this module.
use crate::graphics::{Content, Graphic, VideoClip};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod damage;
mod master;
mod xml;

use crate::project_watch::InputOwner;
pub use damage::{OccurrenceRanges, OutputDamage, ProjectDamage, RevisionChange, TimelineChange};
pub use master::set_v2_enabled;

/// Runtime project together with the writable leaf definitions used by its master.
pub struct AuthoringDocument {
    pub project: Project,
    layout: crate::inspect::Layout,
    details: BTreeMap<String, DetailFile>,
}

struct DetailFile {
    path: PathBuf,
    kind: Detail,
    authored: serde_json::Value,
}

enum Detail {
    Shot(usize),
    Overlay,
    Audio,
}

impl AuthoringDocument {
    /// Validate and resolve an optional unsaved detail in the context of every placement.
    pub fn load(path: &Path, candidate: Option<(&str, &[u8])>) -> Result<Self> {
        Self::load_observed(path, candidate, None, &mut |_, _| {})
    }

    /// Report each direct input before consulting it, including missing detail alternatives.
    pub(crate) fn load_observed(
        path: &Path,
        detail: Option<(&str, &[u8])>,
        master_bytes: Option<&[u8]>,
        observe: &mut dyn FnMut(&Path, InputOwner),
    ) -> Result<Self> {
        let path = Project::entry_path(path);
        observe(&path, InputOwner::Authoring);
        ensure!(
            master_bytes.is_none() || Project::is_master(&path),
            "master edits require a split project master"
        );
        let disk_bytes;
        let bytes = if let Some(bytes) = master_bytes {
            bytes
        } else {
            disk_bytes =
                std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            &disk_bytes
        };
        let base = path.parent().context("project needs a parent directory")?;
        let mut document = if Project::is_master(&path) {
            master::read(base, bytes, detail, &mut |path| {
                observe(path, InputOwner::Authoring)
            })
            .with_context(|| format!("read master {}", path.display()))?
        } else {
            ensure!(
                detail.is_none(),
                "detail edits require a split project master"
            );
            Self {
                project: serde_json::from_slice(bytes)
                    .with_context(|| format!("parse fixture {}", path.display()))?,
                details: BTreeMap::new(),
                layout: crate::inspect::Layout::Single(path.clone()),
            }
        };
        document.project = document.project.resolve_inputs(base, observe)?;
        Ok(document)
    }

    /// The persisted root overlay track state of a split master.
    pub fn root_v2_enabled(&self) -> Option<bool> {
        match &self.layout {
            crate::inspect::Layout::Master {
                root_v2_enabled, ..
            } => Some(*root_v2_enabled),
            crate::inspect::Layout::Single(_) => None,
        }
    }

    pub fn detail_ids(&self) -> impl Iterator<Item = &str> {
        self.details.keys().map(String::as_str)
    }

    /// The CLI and the player inspect the same accepted edit without opening a render device.
    pub fn inspection(&self) -> Result<crate::inspect::Inspection> {
        use crate::inspect::{Definition, Inspection, Layout};
        let (name, placements) = match &self.layout {
            Layout::Master {
                name, placements, ..
            } => (name, placements),
            Layout::Single(file) => return Inspection::single(&self.project, file),
        };
        let mut definitions = BTreeMap::new();
        for (id, file) in &self.details {
            let mut definition = match file.kind {
                Detail::Shot(index) => Definition::shot(&file.path, &self.project.shots[index]),
                Detail::Overlay => {
                    let overlay = self
                        .project
                        .overlays
                        .iter()
                        .find(|p| p.detail == *id)
                        .context("validated V2 definition has no placement")?;
                    Definition::graphics(&file.path, &overlay.graphics)
                }
                Detail::Audio => Definition::graphics(&file.path, &[]),
            };
            definition.source = file
                .authored
                .get("source")
                .and_then(|v| v.as_str())
                .map(PathBuf::from);
            definitions.insert(id.clone(), definition);
        }
        Ok(Inspection {
            name: name.clone(),
            fps: self.project.fps,
            duration: self.project.duration()?,
            placements: placements.clone(),
            definitions,
        })
    }

    /// Project-relative paths of the leaf definitions used by the master.
    pub fn detail_files(&self) -> BTreeMap<&str, &Path> {
        self.details
            .iter()
            .map(|(id, file)| (id.as_str(), file.path.as_path()))
            .collect()
    }

    /// Authored values before dependency paths or placement timing are resolved.
    pub fn detail_value(&self, id: &str) -> Result<&serde_json::Value> {
        Ok(&self
            .details
            .get(id)
            .with_context(|| format!("unplaced detail {id:?}"))?
            .authored)
    }

    pub fn layer(&self, detail: &str, names: &[String]) -> Result<&Graphic> {
        let definition = self
            .details
            .get(detail)
            .with_context(|| format!("unplaced detail {detail:?}"))?;
        let mut children = match &definition.kind {
            Detail::Shot(index) => self.project.shots[*index].graphics.as_slice(),
            Detail::Overlay => self
                .project
                .overlays
                .iter()
                .find(|p| p.detail == detail)
                .context("missing graphics placement")?
                .graphics
                .as_slice(),
            Detail::Audio => anyhow::bail!("text metrics require a picture detail"),
        };
        let mut found = None;
        for name in names {
            let mut matches = children
                .iter()
                .filter(|layer| layer.name.as_ref() == Some(name));
            let layer = matches
                .next()
                .with_context(|| format!("missing layer {name:?} in {detail}"))?;
            ensure!(
                matches.next().is_none(),
                "ambiguous sibling name {name:?} in {detail}"
            );
            children = match &layer.content {
                Content::Group { children } | Content::TransformGroup { children } => {
                    children.as_slice()
                }
                _ => &[],
            };
            found = Some(layer);
        }
        found.context("supply at least one layer name")
    }

    /// CPU preflight of dependencies for an authored edit. Shader compilation uses the render device.
    pub fn validate_assets(&self) -> Result<()> {
        crate::media::assets::PreparedAnimations::prepare(&self.project, |_| None)?.profiles(
            &self.project,
            &Default::default(),
            true,
        )?;
        crate::export::AudioSourceLengths::default().validate(&self.project.audio)?;
        crate::session::preflight_title(&self.project, None)?;
        crate::graphics::validate_document_artwork(&self.project)
    }
}

/// Exact rational frames per second, stored as [numerator, denominator] in this fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FrameRate(pub u32, pub u32);

impl FrameRate {
    /// Invert the exporter's nearest-sample boundaries for a validated,
    /// nonempty project. The end of playback parks on its final picture.
    pub fn frame_at_sample(self, sample: u64, duration: u32) -> u32 {
        let numerator = u128::from(sample) * u128::from(self.0) + u128::from((self.0 - 1) / 2);
        let divisor = u128::from(crate::export::SAMPLE_RATE) * u128::from(self.1);
        (numerator / divisor).min(u128::from(duration - 1)) as u32
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub size: [u32; 2],
    pub fps: FrameRate,
    pub shots: Vec<Shot>,
    /// Graphics placements in paint order, with nested V2 below the enclosing V2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<GraphicsPlacement>,
    #[serde(default, skip_serializing_if = "Title::is_absent")]
    pub title: Title,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<crate::export::AudioClip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<crate::graphics::Graphic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<crate::graphics::Exposure>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphicsPlacement {
    pub detail: String,
    pub start: u32,
    pub duration: u32,
    #[serde(default = "default_enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
    pub graphics: Vec<Graphic>,
}

fn default_enabled() -> bool {
    true
}

fn is_enabled(enabled: &bool) -> bool {
    *enabled
}

impl GraphicsPlacement {
    pub(crate) fn local_frame(&self, frame: u32) -> Option<u32> {
        frame
            .checked_sub(self.start)
            .filter(|&age| age < self.duration)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shot {
    #[serde(default)]
    pub cameras: Vec<crate::camera::Camera>,
    #[serde(default)]
    pub camera: Option<String>,
    #[serde(default)]
    pub camera_cuts: Vec<crate::camera::CameraCut>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub source: Option<PathBuf>,
    #[serde(default)]
    pub source_in: u32,
    pub duration: u32,
    #[serde(default)]
    pub background: Option<Background>,
    /// Shot-local graphics paint above the base and below project-wide graphics.
    #[serde(default)]
    pub graphics: Vec<Graphic>,
}

impl Serialize for Shot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut shot = serializer.serialize_map(None)?;
        if !self.cameras.is_empty() {
            shot.serialize_entry("cameras", &self.cameras)?;
        }
        if let Some(camera) = &self.camera {
            shot.serialize_entry("camera", camera)?;
        }
        if !self.camera_cuts.is_empty() {
            shot.serialize_entry("camera_cuts", &self.camera_cuts)?;
        }
        if let Some(name) = &self.name {
            shot.serialize_entry("name", name)?;
        }
        if let Some(source) = &self.source {
            shot.serialize_entry("source", source)?;
            // Media sessions can address a zero trim through its existing JSON pointer.
            shot.serialize_entry("source_in", &self.source_in)?;
        }
        shot.serialize_entry("duration", &self.duration)?;
        if let Some(background) = &self.background {
            shot.serialize_entry("background", background)?;
        }
        if !self.graphics.is_empty() {
            shot.serialize_entry("graphics", &self.graphics)?;
        }
        shot.end()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Background {
    /// Opaque sRGB color.
    pub color: [u8; 3],
    /// Amplitude of stationary grain in 8-bit sRGB levels.
    #[serde(default)]
    pub grain: u8,
    #[serde(default)]
    pub seed: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Backplate {
    /// Horizontal and vertical pixels around the measured line block.
    pub padding: [u16; 2],
    pub rgba: [u8; 4],
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub text: String,
    pub font: PathBuf,
    pub font_size: u32,
    pub width: u32,
    pub position: [i32; 2],
    pub start: u32,
    pub fade_frames: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backplate: Option<Backplate>,
}

impl Default for Title {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: PathBuf::new(),
            font_size: 16,
            width: 1,
            position: [0, 0],
            start: 0,
            fade_frames: 0,
            backplate: None,
        }
    }
}

impl Title {
    fn is_absent(&self) -> bool {
        self == &Self::default()
    }
}

/// A parent span gates placement, while a video's own start is measured on its
/// owning shot or project clock. None visits all assets, including hidden ones.
pub(crate) fn collect_video_clips<'a>(
    layers: &'a [Graphic],
    frame: Option<u32>,
    clips: &mut Vec<&'a VideoClip>,
) {
    for layer in layers {
        let local = match frame {
            Some(parent) => match layer.local_frame(parent) {
                Some(local) => Some(local),
                None => continue,
            },
            None => None,
        };
        match &layer.content {
            Content::Video(clip) => clips.push(clip),
            Content::Scene3d(scene) => {
                for surface in scene.models.iter().flat_map(|m| &m.surfaces) {
                    collect_video_clips(&surface.layers, local, clips);
                }
            }
            Content::Group { children } | Content::TransformGroup { children } => {
                collect_video_clips(children, local, clips)
            }
            _ => {}
        }
    }
}

impl Project {
    pub(crate) fn entry_path(path: &Path) -> PathBuf {
        if path.is_dir() {
            path.join("project.proj")
        } else {
            path.to_owned()
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(AuthoringDocument::load(path, None)?.project)
    }

    pub(crate) fn is_master(path: &Path) -> bool {
        path.extension().is_some_and(|ext| ext == "proj")
    }

    pub fn duration(&self) -> Result<u32> {
        self.shots.iter().try_fold(0u32, |end, s| {
            end.checked_add(s.duration).context("timeline too long")
        })
    }

    pub(crate) fn graphic_scopes(&self) -> impl Iterator<Item = &[Graphic]> {
        self.local_graphic_scopes()
            .map(|(layers, _)| layers)
            .chain(std::iter::once(self.graphics.as_slice()))
    }

    pub(crate) fn local_graphic_scopes(&self) -> impl Iterator<Item = (&[Graphic], u32)> {
        self.shots
            .iter()
            .map(|s| (s.graphics.as_slice(), s.duration))
            .chain(
                self.overlays
                    .iter()
                    .map(|p| (p.graphics.as_slice(), p.duration)),
            )
    }

    pub(crate) fn graphic_scopes_mut(&mut self) -> impl Iterator<Item = &mut [Graphic]> {
        self.shots
            .iter_mut()
            .map(|s| s.graphics.as_mut_slice())
            .chain(self.overlays.iter_mut().map(|p| p.graphics.as_mut_slice()))
            .chain(std::iter::once(self.graphics.as_mut_slice()))
    }

    pub(crate) fn video_clips(&self) -> Vec<&VideoClip> {
        let mut clips = Vec::new();
        for layers in self.graphic_scopes() {
            collect_video_clips(layers, None, &mut clips);
        }
        clips
    }

    pub(crate) fn resolved(self, base: &Path) -> Result<Self> {
        self.resolve_inputs(base, &mut |_, _| {})
    }

    fn resolve_inputs(
        mut self,
        base: &Path,
        observe: &mut dyn FnMut(&Path, InputOwner),
    ) -> Result<Self> {
        let mut resolve = |path: &mut PathBuf, owner| {
            *path = base.join(&*path);
            observe(path, owner);
        };
        let edit = &mut self;
        for (index, shot) in edit.shots.iter_mut().enumerate() {
            let owner = InputOwner::Shot(index);
            if let Some(path) = &mut shot.source {
                resolve(path, owner);
            }
            crate::graphics::resolve(&mut shot.graphics, &mut |path| resolve(path, owner));
        }
        for (index, overlay) in edit.overlays.iter_mut().enumerate() {
            crate::graphics::resolve(&mut overlay.graphics, &mut |path| {
                resolve(path, InputOwner::Overlay(index))
            });
        }
        if !edit.title.font.as_os_str().is_empty() {
            resolve(&mut edit.title.font, InputOwner::ProjectPicture);
        }
        crate::graphics::resolve(&mut edit.graphics, &mut |path| {
            resolve(path, InputOwner::ProjectPicture)
        });
        for (index, clip) in edit.audio.iter_mut().enumerate() {
            resolve(&mut clip.source, InputOwner::Audio(index));
        }
        edit.validate()?;
        Ok(self)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.size
                .iter()
                .all(|v| (1..=crate::MAX_SURFACE_DIMENSION).contains(v)),
            "invalid output dimensions"
        );
        ensure!(
            self.fps.0 > 0
                && self.fps.1 > 0
                && u64::from(self.fps.0) <= 240 * u64::from(self.fps.1),
            "expected a positive frame rate up to 240 fps"
        );
        ensure!(!self.shots.is_empty(), "project has no shots");
        let end = self.duration()?;
        for shot in &self.shots {
            ensure!(shot.duration > 0, "shot duration must be positive");
            shot.validate_cameras()?;
            ensure!(
                shot.source.is_some() != shot.background.is_some(),
                "shot requires exactly one of source or background"
            );
            ensure!(
                shot.source.is_some() || shot.source_in == 0,
                "source_in requires a media source"
            );
            shot.source_in
                .checked_add(shot.duration)
                .context("source range overflow")?;
            ensure!(
                shot.source.is_some() || crate::graphics::history_depth(&shot.graphics, None) == 0,
                "source history requires a V1 shot with a media source"
            );
        }
        ensure!(
            crate::graphics::history_depth(&self.graphics, None) == 0,
            "place source history on the V1 media shot that owns the source"
        );
        ensure!(
            !crate::camera::contains_scene3d(&self.graphics),
            "scene3d requires a V1 picture placement with a perspective camera"
        );
        for p in &self.overlays {
            ensure!(
                !crate::camera::contains_scene3d(&p.graphics),
                "scene3d requires a V1 picture placement with a perspective camera"
            );
            ensure!(
                p.duration > 0 && p.start.checked_add(p.duration).is_some_and(|v| v <= end),
                "graphics placement {} exceeds project duration",
                p.detail
            );
            ensure!(
                !crate::graphics::uses_footage(&p.graphics),
                "graphics placement {} has no primary footage; use a video layer",
                p.detail
            );
        }
        for (layers, duration) in self.local_graphic_scopes() {
            crate::graphics::validate(layers, duration)?;
        }
        ensure!(
            (1..=crate::MAX_SURFACE_DIMENSION).contains(&self.title.width),
            "invalid paragraph width"
        );
        ensure!(
            (1..=512).contains(&self.title.font_size),
            "invalid font size"
        );
        crate::graphics::validate(&self.graphics, end)?;
        if let Some(exposure) = self.exposure {
            exposure.validate()?;
        }
        crate::export::validate_audio(&self.audio, self.fps, end)
    }
}
