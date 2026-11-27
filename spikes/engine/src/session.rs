//! An edit session retains the accepted authoring document, including relative asset paths.
//! Disk bytes form a separate baseline so an external edit can be detected before saving.
use crate::{Engine, Project};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Session {
    file: PathBuf,
    disk_bytes: Vec<u8>,
    disk_document: Project,
    accepted: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    engine: Engine,
    audio_lengths: crate::export::AudioSourceLengths,
}

impl Session {
    pub fn open(path: &Path) -> Result<Self> {
        let path = Project::entry_path(path);
        let file = path
            .canonicalize()
            .with_context(|| format!("open {}", path.display()))?;
        ensure!(
            !Project::is_master(&path) && !Project::is_master(&file),
            "split project: edit the .proj master or its sequence details directly, then reopen; session save currently supports single-file JSON"
        );
        let disk_bytes = fs::read(&file)?;
        let accepted: Project = serde_json::from_slice(&disk_bytes)?;
        let edit = accepted
            .clone()
            .resolved(file.parent().context("missing project directory")?)?;
        let mut audio_lengths = crate::export::AudioSourceLengths::default();
        audio_lengths.validate(&edit.audio)?;
        let mut engine = Engine::new(edit)?;
        engine.prepare_session_artwork()?;
        Ok(Self {
            file,
            disk_bytes,
            disk_document: accepted.clone(),
            accepted,
            undo: Vec::new(),
            redo: Vec::new(),
            engine,
            audio_lengths,
        })
    }

    pub fn revision(&self) -> u64 {
        self.engine.revision()
    }

    pub fn render(&mut self, index: u32) -> Result<crate::Frame> {
        self.engine.render(index)
    }

    /// Reopen a dependency edited in place, including an SVG, font or shader.
    /// Linked producer output that must survive interruption uses relink instead.
    pub fn refresh_assets(&mut self, expected: u64) -> Result<()> {
        self.check(expected)?;
        let next = self
            .accepted
            .clone()
            .resolved(self.file.parent().context("missing project directory")?)?;
        let mut lengths = crate::export::AudioSourceLengths::default();
        lengths.validate(&next.audio)?;
        let mut engine = Engine::new(next)?;
        engine.prepare_session_artwork()?;
        engine.revision = self
            .revision()
            .checked_add(1)
            .context("revision counter exhausted")?;
        self.engine = engine;
        self.audio_lengths = lengths;
        Ok(())
    }

    fn check(&self, expected: u64) -> Result<()> {
        ensure!(
            expected == self.revision(),
            "stale edit at revision {expected}; current revision {}",
            self.revision()
        );
        Ok(())
    }

    fn install(&mut self, doc: &Project) -> Result<()> {
        let edit = doc
            .clone()
            .resolved(self.file.parent().context("missing project directory")?)?;
        preflight_title(&edit, Some(self.engine.project()))?;
        if self.engine.project().audio != edit.audio {
            self.audio_lengths.validate(&edit.audio)?;
        }
        self.engine.replace_project(edit)
    }

    fn accept(&mut self, doc: Project) -> Result<()> {
        if doc == self.accepted {
            return Ok(());
        }
        self.install(&doc)?;
        self.undo.push(std::mem::replace(&mut self.accepted, doc));
        self.redo.clear();
        Ok(())
    }

    /// Change an existing field addressed by a JSON Pointer in the provisional syntax.
    pub fn set(&mut self, expected: u64, pointer: &str, value: Value) -> Result<()> {
        self.check(expected)?;
        let mut json = serde_json::to_value(&self.accepted)?;
        *json
            .pointer_mut(pointer)
            .with_context(|| format!("unknown edit field {pointer}"))? = value;
        self.accept(serde_json::from_value(json)?)
    }

    pub fn undo(&mut self, expected: u64) -> Result<()> {
        self.check(expected)?;
        let previous = self.undo.last().context("nothing to undo")?.clone();
        self.install(&previous)?;
        self.undo.pop();
        self.redo
            .push(std::mem::replace(&mut self.accepted, previous));
        Ok(())
    }

    /// Publish a completed render once for every picture use of its predecessor.
    /// Producers write a fresh immutable file for each accepted version.
    pub fn relink(&mut self, expected: u64, from: &Path, to: &Path) -> Result<()> {
        self.check(expected)?;
        ensure!(from != to, "a refreshed source needs a new asset path");
        let mut next = self.accepted.clone();
        let mut found = false;
        for shot in &mut next.shots {
            if shot.source.as_deref() == Some(from) {
                shot.source = Some(to.to_owned());
                found = true;
            }
        }
        for layers in next.graphic_scopes_mut() {
            relink_video_layers(layers, from, to, &mut found);
        }
        ensure!(found, "no video sources use {}", from.display());
        self.accept(next)
    }

    pub fn redo(&mut self, expected: u64) -> Result<()> {
        self.check(expected)?;
        let next = self.redo.last().context("nothing to redo")?.clone();
        self.install(&next)?;
        self.redo.pop();
        self.undo.push(std::mem::replace(&mut self.accepted, next));
        Ok(())
    }

    /// Reload a complete external edit. On conflict the disk and in-memory versions survive.
    pub fn reload(&mut self, expected: u64) -> Result<()> {
        self.check(expected)?;
        let bytes = fs::read(&self.file)?;
        if bytes == self.disk_bytes {
            return Ok(());
        }
        let next: Project =
            serde_json::from_slice(&bytes).context("incomplete or invalid external edit")?;
        if next != self.disk_document
            && self.accepted != self.disk_document
            && next != self.accepted
        {
            bail!(
                "external edit conflicts with unsaved revision {}",
                self.revision()
            );
        }
        if next != self.disk_document {
            self.accept(next.clone())?;
        }
        self.disk_document = next;
        self.disk_bytes = bytes;
        Ok(())
    }

    /// Atomic replacement on the same filesystem. Cooperating session writers serialize
    /// their check and rename; ordinary text editors do not participate in the advisory lock.
    pub fn save(&mut self, expected: u64) -> Result<()> {
        self.check(expected)?;
        let mut bytes = serde_json::to_vec_pretty(&self.accepted)?;
        bytes.push(b'\n');
        write_document(&self.file, &self.disk_bytes, &bytes, || Ok(()))?;
        self.disk_document = self.accepted.clone();
        self.disk_bytes = bytes;
        Ok(())
    }
}

/// Serialize cooperating writers and check the baseline immediately before publishing.
pub(crate) fn write_document(
    file: &Path,
    baseline: &[u8],
    bytes: &[u8],
    verify_inputs: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let parent = file.parent().context("missing project directory")?;
    let mut lock_name = file
        .file_name()
        .context("missing project name")?
        .to_os_string();
    lock_name.push(".lock");
    let lock = fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(lock_name))?;
    lock.lock()?;
    let pending = if bytes != baseline {
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.as_file()
            .set_permissions(fs::metadata(file)?.permissions())?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        Some(temp)
    } else {
        None
    };
    verify_inputs()?;
    ensure!(
        fs::read(file)? == baseline,
        "project changed on disk; reload before saving"
    );
    if let Some(temp) = pending {
        temp.persist(file)?;
    }
    Ok(())
}

fn relink_video_layers(
    layers: &mut [crate::graphics::Graphic],
    from: &Path,
    to: &Path,
    found: &mut bool,
) {
    use crate::graphics::Content;
    for layer in layers {
        match &mut layer.content {
            Content::Video(clip) if clip.source == from => {
                clip.source = to.to_owned();
                *found = true;
            }
            Content::Group { children } | Content::TransformGroup { children } => {
                relink_video_layers(children, from, to, found)
            }
            Content::Scene3d(scene) => {
                for surface in scene.models.iter_mut().flat_map(|m| &mut m.surfaces) {
                    relink_video_layers(&mut surface.layers, from, to, found);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn preflight_title(edit: &Project, old: Option<&Project>) -> Result<()> {
    if old.is_none_or(|p| {
        p.title.text != edit.title.text
            || p.title.font != edit.title.font
            || p.title.width != edit.title.width
            || p.title.font_size != edit.title.font_size
            || p.title.backplate != edit.title.backplate
    }) {
        crate::render::text::paint_project_title(&edit.title)?;
    }
    Ok(())
}
