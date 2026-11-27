//! The authored structure of one accepted project revision. Occurrence paths locate
//! individual uses; definitions and property keys are shared across those uses.
use crate::project_watch::InputOwner;
use crate::{FrameRate, Project, Shot, camera::CameraCut, graphics::Graphic};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod tracks;
pub use tracks::{Layer, Track};

pub(crate) enum Layout {
    Master {
        name: String,
        placements: Vec<Placement>,
        root_v2_enabled: bool,
    },
    Single(PathBuf),
}

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub name: String,
    pub fps: FrameRate,
    pub duration: u32,
    pub placements: Vec<Placement>,
    pub definitions: BTreeMap<String, Definition>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Placement {
    pub name: String,
    /// Address within this revision. Includes lane, definition and start in the enclosing scope.
    pub path: String,
    pub track: String,
    /// Disabled placements retain their address and the timing shown in the editor.
    pub enabled: bool,
    /// Half-open range in the enclosing editorial sequence.
    pub local: [u32; 2],
    /// Half-open range in the complete film.
    pub project: [u32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Placement>,
    /// Resolved input for this occurrence in this snapshot only.
    #[serde(skip)]
    pub(crate) owner: Option<InputOwner>,
}

impl Placement {
    pub(crate) fn new(
        parent: &str,
        origin: u32,
        name: &str,
        track: &str,
        local: [u32; 2],
        definition: Option<String>,
    ) -> Result<Self> {
        let component = format!("{track}:{name}@{}", local[0]);
        let path = if parent.is_empty() {
            component
        } else {
            format!("{parent}/{component}")
        };
        let start = origin
            .checked_add(local[0])
            .context("placement start exceeds project clock")?;
        let end = origin
            .checked_add(local[1])
            .context("placement end exceeds project clock")?;
        Ok(Self {
            name: name.into(),
            path,
            track: track.into(),
            enabled: true,
            local,
            project: [start, end],
            definition,
            children: vec![],
            owner: None,
        })
    }

    pub(crate) fn set_cuts(&mut self, cuts: &[CameraCut]) -> Result<()> {
        self.children = cuts
            .iter()
            .map(|cut| {
                Self::new(
                    &self.path,
                    self.project[0],
                    &cut.camera,
                    "CAM",
                    [cut.start, cut.end],
                    None,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct Definition {
    /// Project-relative file holding the source definition.
    pub file: PathBuf,
    /// Media used by a footage or audio detail, where present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_camera: Option<String>,
    pub layers: Vec<Layer>,
}

impl Definition {
    pub(crate) fn graphics(file: &Path, graphics: &[Graphic]) -> Self {
        Self {
            file: file.into(),
            source: None,
            default_camera: None,
            layers: tracks::layers(graphics, &[]),
        }
    }
    pub(crate) fn shot(file: &Path, shot: &Shot) -> Self {
        Self {
            file: file.into(),
            source: shot.source.clone(),
            default_camera: shot.camera.clone(),
            layers: tracks::layers(&shot.graphics, &shot.cameras),
        }
    }

    /// Resolve an authored property by its layer path. Repeated sibling names
    /// are valid, so a by-name query reports ambiguity instead of guessing.
    pub fn property(&self, names: &[String], property: &str) -> Result<(&Layer, &Track)> {
        let mut siblings = self.layers.as_slice();
        let mut owner = None;
        for name in names {
            let mut matches = siblings.iter().filter(|layer| layer.name == *name);
            let layer = matches
                .next()
                .with_context(|| format!("unknown layer {name:?}"))?;
            ensure!(matches.next().is_none(), "ambiguous sibling name {name:?}");
            owner = Some(layer);
            siblings = &layer.children;
        }
        let owner = owner.context("choose the layer that owns the property")?;
        let track = owner
            .tracks
            .iter()
            .find(|track| track.name == property)
            .with_context(|| {
                format!("unknown animated property {property:?} on {:?}", owner.name)
            })?;
        Ok((owner, track))
    }
}

impl Inspection {
    pub fn placement(&self, path: &str) -> Option<&Placement> {
        let mut siblings = self.placements.as_slice();
        for (idx, _) in path.match_indices('/') {
            siblings = &siblings.iter().find(|p| p.path == path[..idx])?.children;
        }
        siblings.iter().find(|p| p.path == path)
    }

    pub fn uses(&self, definition: &str) -> Vec<&Placement> {
        fn visit<'a>(placements: &'a [Placement], id: &str, result: &mut Vec<&'a Placement>) {
            for p in placements {
                if p.definition.as_deref() == Some(id) {
                    result.push(p);
                }
                visit(&p.children, id, result);
            }
        }
        let mut result = Vec::new();
        visit(&self.placements, definition, &mut result);
        result
    }

    /// A single-file fixture has a shot list and project-wide artwork. It carries
    /// no editorial parentage. Preserve which definition each list entry owns.
    pub(crate) fn single(project: &Project, entry: &Path) -> Result<Self> {
        let file = Path::new(entry.file_name().context("missing document filename")?);
        let base = entry.parent().context("missing document directory")?;
        let mut inspection = Self {
            name: file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            fps: project.fps,
            duration: project.duration()?,
            placements: vec![],
            definitions: BTreeMap::new(),
        };
        let mut cursor = 0;
        for (i, shot) in project.shots.iter().enumerate() {
            let id = format!("shot-{}", i + 1);
            let end = cursor + shot.duration;
            let mut item = Placement::new("", 0, &id, "V1", [cursor, end], Some(id.clone()))?;
            item.owner = Some(InputOwner::Shot(i));
            let mut definition = Definition::shot(file, shot);
            if let Some(source) = &shot.source {
                definition.source = Some(source.strip_prefix(base).unwrap_or(source).into());
            }
            if let Some(name) = &shot.name {
                item.name.clone_from(name);
            }
            item.set_cuts(&shot.camera_cuts)?;
            inspection.placements.push(item);
            inspection.definitions.insert(id, definition);
            cursor = end;
        }
        for (i, overlay) in project.overlays.iter().enumerate() {
            let id = format!("overlay-{}", i + 1);
            let range = [overlay.start, overlay.start + overlay.duration];
            let mut item = Placement::new("", 0, &id, "V2", range, Some(id.clone()))?;
            item.owner = Some(InputOwner::Overlay(i));
            item.enabled = overlay.enabled;
            item.name.clone_from(&overlay.detail);
            inspection.placements.push(item);
            inspection
                .definitions
                .insert(id, Definition::graphics(file, &overlay.graphics));
        }
        if !project.graphics.is_empty() {
            let id = "graphics";
            inspection.placements.push(Placement::new(
                "",
                0,
                id,
                "V2",
                [0, inspection.duration],
                Some(id.into()),
            )?);
            inspection
                .definitions
                .insert(id.into(), Definition::graphics(file, &project.graphics));
        }
        if !project.title.text.is_empty() && project.title.start < inspection.duration {
            let mut item = Placement::new(
                "",
                0,
                "title",
                "V2",
                [project.title.start, inspection.duration],
                Some("title".into()),
            )?;
            item.name.clone_from(&project.title.text);
            inspection.placements.push(item);
            inspection
                .definitions
                .insert("title".into(), Definition::graphics(file, &[]));
        }
        for (i, audio) in project.audio.iter().enumerate() {
            let id = format!("audio-{}", i + 1);
            let frame = |sample| project.fps.frame_at_sample(sample, inspection.duration);
            let range = [
                frame(audio.start_sample),
                frame(audio.start_sample + audio.samples - 1) + 1,
            ];
            let mut item = Placement::new("", 0, &id, "A1", range, Some(id.clone()))?;
            item.owner = Some(InputOwner::Audio(i));
            item.name = audio
                .source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            inspection.placements.push(item);
            let mut definition = Definition::graphics(file, &[]);
            definition.source = Some(
                audio
                    .source
                    .strip_prefix(base)
                    .unwrap_or(&audio.source)
                    .into(),
            );
            inspection.definitions.insert(id, definition);
        }
        inspection.placements.sort_by_key(|p| p.project[0]);
        Ok(inspection)
    }
}
