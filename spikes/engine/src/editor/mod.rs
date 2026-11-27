//! The state and commands of one open, editable project.
mod geometry;
pub mod hud;

pub use geometry::{GeometryClip, HitTest, LayerGeometry, LayerRef, Visibility};

use crate::{
    Frame, Project, RevisionChange,
    host::{PreparedSnapshot, Viewer},
    inspect::Inspection,
    project::set_v2_enabled,
    project_watch::{ProjectInputs, ProjectWatch},
    session::write_document,
};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const ROOT: &str = "root";

// TODO(editor): Address a mask under its owner and add targets for scene
// objects, cameras, and lights when their edit commands are defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    Layer {
        #[serde(flatten)]
        layer: LayerRef,
    },
    Placement {
        occurrence: String,
    },
    Track {
        scope: String,
        track: String,
    },
}

// TODO(editor): Define a layer move intent in parent coordinates, including
// base-value and keyframe edits, with one history entry per gesture.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditorCommand {
    GetState,
    Inspect,
    Seek {
        frame: u32,
    },
    EnterScope {
        revision: u64,
        scope: String,
    },
    HitTest {
        revision: u64,
        frame: u32,
        point: [f64; 2],
    },
    SelectAt {
        revision: u64,
        frame: u32,
        point: [f64; 2],
    },
    HoverAt {
        revision: u64,
        frame: u32,
        point: [f64; 2],
    },
    ClearHover {
        revision: u64,
    },
    BeginMarquee {
        revision: u64,
        frame: u32,
        point: [f64; 2],
        threshold: Option<f64>,
    },
    UpdateMarquee {
        revision: u64,
        point: [f64; 2],
    },
    FinishMarquee {
        revision: u64,
        point: [f64; 2],
    },
    CancelMarquee {
        revision: u64,
    },
    LayerGeometry {
        revision: u64,
        frame: u32,
        layer: LayerRef,
    },
    Select {
        revision: u64,
        target: Target,
    },
    ClearSelection,
    SetTrackEnabled {
        revision: u64,
        scope: String,
        track: String,
        enabled: bool,
    },
    Undo {
        revision: u64,
    },
    Redo {
        revision: u64,
    },
    Save {
        revision: u64,
    },
    Reload {
        revision: u64,
        #[serde(default)]
        discard_changes: bool,
    },
    Render {
        revision: u64,
        frame: u32,
        #[serde(default)]
        points: Vec<[u32; 2]>,
    },
}

#[derive(Debug, Serialize)]
pub struct EditorError {
    pub code: &'static str,
    pub message: String,
}

impl EditorError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn operation(error: anyhow::Error) -> Self {
        Self::new("operationFailed", format!("{error:#}"))
    }
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.write_str(&self.message)
    }
}

impl std::error::Error for EditorError {}

#[derive(Debug, Serialize)]
pub struct TrackState {
    pub scope: String,
    pub track: String,
    pub enabled: bool,
    pub editable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub session_id: String,
    pub revision: u64,
    pub hud_version: u64,
    pub frame: u32,
    pub scope: String,
    pub selection: Option<Target>,
    pub selections: Vec<Target>,
    pub selection_source: Option<SourceAddress>,
    pub marquee: Option<[f64; 4]>,
    pub hover: Option<LayerRef>,
    pub hud: Option<LayerGeometry>,
    pub tracks: Vec<TrackState>,
    pub dirty: bool,
    pub conflict: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Serialize)]
pub struct SourceAddress {
    pub file: PathBuf,
    pub layers: Vec<usize>,
}

#[derive(Serialize)]
struct EditResult {
    state: EditorState,
    change: Option<RevisionChange>,
    #[serde(skip)]
    picture: Option<Frame>,
}

pub struct EditorReply {
    pub result: Value,
    pub transition: Option<(Frame, RevisionChange)>,
}

type Master = Arc<[u8]>;

#[derive(Clone, Copy)]
struct Marquee {
    frame: u32,
    anchor: [f64; 2],
    point: [f64; 2],
    threshold: f64,
    dragging: bool,
}

impl Marquee {
    fn move_to(&mut self, point: [f64; 2]) {
        self.point = point;
        self.dragging |=
            (point[0] - self.anchor[0]).hypot(point[1] - self.anchor[1]) >= self.threshold;
    }

    fn bounds(self) -> Option<[f64; 4]> {
        let [x, y] = self.anchor;
        let [u, v] = self.point;
        self.dragging
            .then_some([x.min(u), y.min(v), x.max(u), y.max(v)])
    }
}

/// Holds the accepted renderer and the written baseline separately from the draft.
pub struct EditorSession {
    path: PathBuf,
    viewer: Viewer,
    coverage_cache: geometry::NativeCoverageCache,
    watch: ProjectWatch,
    saved_inputs: ProjectInputs,
    session_id: String,
    hud_version: u64,
    frame: u32,
    scope: String,
    selections: Vec<Target>,
    marquee: Option<Marquee>,
    hover: Option<(u32, LayerRef)>,
    conflict: bool,
    saved: Master,
    working: Master,
    undo: Vec<Master>,
    redo: Vec<Master>,
}

impl EditorSession {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let path = Project::entry_path(path).canonicalize()?;
        ensure!(
            Project::is_master(&path),
            "editor requires a split project master"
        );
        let saved: Master = fs::read(&path)?.into();
        let viewer = Viewer::open(&path, 0)?;
        let saved_inputs = viewer.snapshot.inputs.clone();
        saved_inputs.check()?;
        ensure!(&*saved == fs::read(&path)?, "master changed while opening");
        let watch = ProjectWatch::new(&saved_inputs)?;
        let mut id = [0; 16];
        getrandom::fill(&mut id).context("identify editor session")?;
        Ok(Self {
            path,
            viewer,
            coverage_cache: geometry::NativeCoverageCache::default(),
            watch,
            saved_inputs,
            session_id: format!("{:032x}", u128::from_be_bytes(id)),
            hud_version: 0,
            frame: 0,
            scope: ROOT.into(),
            selections: vec![],
            marquee: None,
            hover: None,
            conflict: false,
            saved: Arc::clone(&saved),
            working: saved,
            undo: vec![],
            redo: vec![],
        })
    }

    pub fn state(&self) -> EditorState {
        let selection = self.selections.first();
        let hud = match selection {
            Some(Target::Layer { layer }) => self.geometry(self.frame, layer).ok(),
            _ => None,
        };
        EditorState {
            session_id: self.session_id.clone(),
            revision: self.viewer.snapshot.revision,
            hud_version: self.hud_version,
            frame: self.frame,
            scope: self.scope.clone(),
            selection: selection.cloned(),
            selections: self.selections.clone(),
            selection_source: selection.and_then(|target| self.selection_source(target)),
            marquee: self.marquee.and_then(Marquee::bounds),
            hover: self.hover.as_ref().map(|(_, layer)| layer.clone()),
            hud,
            tracks: self.tracks(&self.scope),
            dirty: self.dirty(),
            conflict: self.conflict,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
        }
    }

    pub fn inspection(&self) -> &Inspection {
        &self.viewer.inspection
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The host's preview transport uses this same accepted renderer.
    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    pub fn viewer_mut(&mut self) -> &mut Viewer {
        &mut self.viewer
    }

    pub fn take_input_change(&self) -> bool {
        self.watch.take_changed()
    }

    pub fn hud_version(&self) -> u64 {
        self.hud_version
    }

    pub fn set_playhead(&mut self, frame: u32) -> Result<(), EditorError> {
        self.frame_in_project(frame)?;
        if self.frame != frame {
            self.frame = frame;
            self.marquee = None;
            self.hover = None;
            self.hud_version += 1;
        }
        Ok(())
    }

    /// Select a picture time independently of the transient scrub preview.
    pub fn render_hud(&self, frame: u32) -> Result<Option<hud::HudImage>, EditorError> {
        self.frame_in_project(frame)?;
        let geometry = self
            .selections
            .iter()
            .filter_map(|target| match target {
                Target::Layer { layer } => Some(self.geometry(frame, layer)),
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        let marquee = self
            .marquee
            .filter(|gesture| gesture.frame == frame)
            .and_then(Marquee::bounds);
        let hover = self
            .hover
            .as_ref()
            .filter(|(hover_frame, _)| *hover_frame == frame)
            .map(|(_, layer)| self.geometry(frame, layer))
            .transpose()?;
        hud::render_selection(&geometry, marquee, hover.as_ref(), self.project().size)
            .map_err(EditorError::operation)
    }

    fn project(&self) -> &Project {
        &self.viewer.snapshot.document.project
    }

    fn tracks(&self, scope: &str) -> Vec<TrackState> {
        let placements = if scope == ROOT {
            self.inspection().placements.as_slice()
        } else if let Some(placement) = self.inspection().placement(scope) {
            placement.children.as_slice()
        } else {
            return Vec::new();
        };
        let tracks: BTreeSet<_> = placements
            .iter()
            .map(|placement| &placement.track)
            .collect();
        tracks
            .into_iter()
            .map(|track| {
                let editable = scope == ROOT && track == "V2";
                TrackState {
                    scope: scope.into(),
                    track: track.clone(),
                    enabled: !editable
                        || self
                            .viewer
                            .snapshot
                            .document
                            .root_v2_enabled()
                            .unwrap_or(true),
                    editable,
                }
            })
            .collect()
    }

    fn dirty(&self) -> bool {
        self.working != self.saved
    }

    fn check_revision(&self, revision: u64) -> Result<(), EditorError> {
        let current = self.viewer.snapshot.revision;
        if revision != current {
            return Err(EditorError::new(
                "staleRevision",
                format!("revision {revision} is stale, current revision is {current}"),
            ));
        }
        Ok(())
    }

    fn frame_in_project(&self, frame: u32) -> Result<(), EditorError> {
        self.viewer
            .check_frame(frame)
            .map_err(|error| EditorError::new("invalidFrame", error.to_string()))
    }

    fn geometry(&self, frame: u32, layer: &LayerRef) -> Result<LayerGeometry, EditorError> {
        self.frame_in_project(frame)?;
        geometry::resolve(
            self.project(),
            self.inspection(),
            self.geometry_assets(),
            frame,
            layer,
        )
        .map_err(EditorError::operation)
    }

    fn geometry_assets(&self) -> geometry::GeometryAssets<'_> {
        geometry::GeometryAssets {
            svg: &self.viewer.snapshot.svg_assets,
            text: &self.viewer.snapshot.text_layouts,
            images: &self.viewer.snapshot.image_assets,
            canvas: self.project().size,
            native: &self.coverage_cache,
        }
    }

    fn valid_target(&self, target: &Target) -> bool {
        match target {
            Target::Layer { layer } => {
                geometry::layer_exists(self.project(), self.inspection(), layer)
            }
            Target::Placement { occurrence } => self.inspection().placement(occurrence).is_some(),
            Target::Track { scope, track } => {
                self.tracks(scope).iter().any(|lane| lane.track == *track)
            }
        }
    }

    fn change_selection(&mut self, targets: Vec<Target>) {
        self.cancel_marquee();
        if targets != self.selections {
            self.selections = targets;
            if let Some((_, layer)) = &self.hover
                && self.layer_selected(layer)
            {
                self.hover = None;
            }
            self.hud_version += 1;
        }
    }

    fn layer_selected(&self, layer: &LayerRef) -> bool {
        self.selections.iter().any(|target| match target {
            Target::Layer { layer: selected } => selected == layer,
            _ => false,
        })
    }

    fn hit_test(&self, frame: u32, point: [f64; 2]) -> Result<HitTest, EditorError> {
        self.frame_in_project(frame)?;
        geometry::hit_test(
            self.project(),
            self.inspection(),
            self.geometry_assets(),
            frame,
            point,
        )
        .map_err(EditorError::operation)
    }

    fn select_at(&mut self, frame: u32, point: [f64; 2]) -> Result<(), EditorError> {
        let hit = self.hit_test(frame, point)?;
        if hit.hits.is_empty() && hit.indeterminate {
            return Err(EditorError::new(
                "indeterminateHit",
                "unsupported coverage may hide the selection target",
            ));
        }
        let target = hit
            .hits
            .into_iter()
            .next()
            .map(|layer| Target::Layer { layer })
            .or_else(|| {
                hit.fallback_placement
                    .map(|occurrence| Target::Placement { occurrence })
            });
        self.change_selection(target.into_iter().collect());
        Ok(())
    }

    fn change_hover(&mut self, hover: Option<(u32, LayerRef)>) {
        if hover != self.hover {
            self.hover = hover;
            self.hud_version += 1;
        }
    }

    fn hover_at(&mut self, frame: u32, point: [f64; 2]) -> Result<(), EditorError> {
        if self.marquee.is_some() {
            return Ok(());
        }
        self.frame_in_project(frame)?;
        let hit = geometry::hover_target(
            self.project(),
            self.inspection(),
            self.geometry_assets(),
            frame,
            point,
        )
        .map_err(EditorError::operation)?;
        self.change_hover(
            hit.filter(|layer| !self.layer_selected(layer))
                .map(|layer| (frame, layer)),
        );
        Ok(())
    }

    fn marquee_point(&self, point: [f64; 2]) -> Result<[f64; 2], EditorError> {
        if !point.iter().all(|value| value.is_finite()) {
            return Err(EditorError::new(
                "invalidPoint",
                "pointer coordinates must be finite",
            ));
        }
        let [w, h] = self.project().size.map(f64::from);
        Ok([point[0].clamp(0., w), point[1].clamp(0., h)])
    }

    fn begin_marquee(
        &mut self,
        frame: u32,
        point: [f64; 2],
        threshold: Option<f64>,
    ) -> Result<(), EditorError> {
        self.frame_in_project(frame)?;
        let point = self.marquee_point(point)?;
        let threshold = threshold.unwrap_or(3.);
        if !threshold.is_finite() || threshold < 0. {
            return Err(EditorError::new(
                "invalidThreshold",
                "drag threshold must be finite and nonnegative",
            ));
        }
        self.cancel_marquee();
        self.change_hover(None);
        self.marquee = Some(Marquee {
            frame,
            anchor: point,
            point,
            threshold,
            dragging: false,
        });
        Ok(())
    }

    fn update_marquee(&mut self, point: [f64; 2]) -> Result<(), EditorError> {
        let point = self.marquee_point(point)?;
        if let Some(gesture) = &mut self.marquee {
            let previous = gesture.bounds();
            gesture.move_to(point);
            if previous != gesture.bounds() {
                self.hud_version += 1;
            }
        }
        Ok(())
    }

    fn finish_marquee(&mut self, point: [f64; 2]) -> Result<(), EditorError> {
        let point = self.marquee_point(point)?;
        let Some(mut gesture) = self.marquee else {
            return Ok(());
        };
        self.cancel_marquee();
        gesture.move_to(point);
        let Some(area) = gesture.bounds() else {
            return self.select_at(gesture.frame, point);
        };
        let layers = geometry::marquee_hits(
            self.project(),
            self.inspection(),
            self.geometry_assets(),
            gesture.frame,
            area,
        )
        .map_err(EditorError::operation)?;
        self.change_selection(
            layers
                .into_iter()
                .map(|layer| Target::Layer { layer })
                .collect(),
        );
        Ok(())
    }

    fn cancel_marquee(&mut self) {
        if self.marquee.take().and_then(Marquee::bounds).is_some() {
            self.hud_version += 1;
        }
    }

    fn selection_source(&self, target: &Target) -> Option<SourceAddress> {
        let (occurrence, layers) = match target {
            Target::Layer { layer } => (&layer.occurrence, layer.layers.clone()),
            Target::Placement { .. } | Target::Track { .. } => {
                return Some(SourceAddress {
                    file: self.path.file_name()?.into(),
                    layers: vec![],
                });
            }
        };
        let definition = self
            .inspection()
            .placement(occurrence)?
            .definition
            .as_ref()?;
        let file = self.inspection().definitions.get(definition)?.file.clone();
        Some(SourceAddress { file, layers })
    }

    fn check_disk(&self) -> anyhow::Result<()> {
        self.saved_inputs.check()?;
        ensure!(
            fs::read(&self.path)? == &*self.saved,
            "master changed on disk"
        );
        Ok(())
    }

    fn disk_conflict(&mut self) -> EditorError {
        if !self.conflict {
            self.conflict = true;
            self.hud_version += 1;
        }
        EditorError::new(
            "diskConflict",
            "project inputs changed on disk; the working edit is still open",
        )
    }

    fn result(&self, transition: Option<(Frame, RevisionChange)>) -> EditResult {
        let (picture, change) = match transition {
            Some((picture, change)) => (Some(picture), Some(change)),
            None => (None, None),
        };
        EditResult {
            state: self.state(),
            change,
            picture,
        }
    }

    fn prepare(&self, bytes: &[u8]) -> anyhow::Result<(RevisionChange, PreparedSnapshot)> {
        let (document, inputs) = ProjectInputs::load_with_master(&self.path, bytes)?;
        PreparedSnapshot::candidate(
            document,
            inputs,
            &self.viewer.snapshot,
            Some(&self.viewer.prepared),
        )
    }

    fn activate(
        &mut self,
        next: PreparedSnapshot,
        change: RevisionChange,
        external: bool,
    ) -> anyhow::Result<(Frame, RevisionChange)> {
        // A position in a changed layer tree has no durable identity. Compare
        // the containing authored branch before retaining it after reload.
        // TODO(editor): Attribute external changes to authored properties and
        // keys so the preview can highlight the affected layer.
        let retained: Vec<_> = self
            .selections
            .iter()
            .filter(|target| {
                !external
                    || match target {
                        Target::Layer { layer } => {
                            let old =
                                geometry::unique_branch(self.project(), self.inspection(), layer);
                            let new = geometry::unique_branch(
                                &next.snapshot.document.project,
                                &next.inspection,
                                layer,
                            );
                            old.is_some() && old == new
                        }
                        _ => true,
                    }
            })
            .cloned()
            .collect();
        let (parked, change) = self
            .viewer
            .accept_candidate(next, change, Some(self.frame))?;
        self.frame = parked.trace.request.timeline_frame;
        if self.scope != ROOT && self.inspection().placement(&self.scope).is_none() {
            self.scope = ROOT.into();
        }
        self.selections = retained
            .into_iter()
            .filter(|target| self.valid_target(target))
            .collect();
        self.marquee = None;
        self.hover = None;
        self.hud_version += 1;
        Ok((parked, change))
    }

    fn install_edit(&mut self, bytes: Master) -> Result<(Frame, RevisionChange), EditorError> {
        if self.check_disk().is_err() {
            return Err(self.disk_conflict());
        }
        let candidate = self.prepare(&bytes);
        if self.check_disk().is_err() {
            return Err(self.disk_conflict());
        }
        let (change, next) = candidate.map_err(EditorError::operation)?;
        let transition = self
            .activate(next, change, false)
            .map_err(EditorError::operation)?;
        self.working = bytes;
        Ok(transition)
    }

    fn toggle_v2(
        &mut self,
        scope: &str,
        track: &str,
        enabled: bool,
    ) -> Result<EditResult, EditorError> {
        // TODO(editor): Extend edits to nested tracks and V1 visibility.
        // Distinguish authored audio mute from monitor mute.
        let Some(lane) = self
            .tracks(scope)
            .into_iter()
            .find(|lane| lane.track == track && lane.editable)
        else {
            return Err(EditorError::new(
                "unsupportedTrack",
                "the first editor slice can edit the root V2 track",
            ));
        };
        if lane.enabled == enabled {
            return Ok(self.result(None));
        }
        let bytes = set_v2_enabled(&self.working, enabled).map_err(EditorError::operation)?;
        let previous = Arc::clone(&self.working);
        let transition = self.install_edit(bytes.into())?;
        self.undo.push(previous);
        self.redo.clear();
        Ok(self.result(Some(transition)))
    }

    fn step_history(&mut self, undo: bool) -> Result<EditResult, EditorError> {
        let source = if undo { &self.undo } else { &self.redo };
        let Some(bytes) = source.last().cloned() else {
            return Ok(self.result(None));
        };
        let previous = Arc::clone(&self.working);
        let transition = self.install_edit(bytes)?;
        if undo {
            self.undo.pop();
            self.redo.push(previous);
        } else {
            self.redo.pop();
            self.undo.push(previous);
        }
        Ok(self.result(Some(transition)))
    }

    fn save(&mut self) -> Result<EditorState, EditorError> {
        if self.check_disk().is_err() {
            return Err(self.disk_conflict());
        }
        if !self.dirty() {
            return Ok(self.state());
        }
        let result = write_document(&self.path, &self.saved, &self.working, || {
            self.saved_inputs.check()
        });
        if let Err(error) = result {
            return if self.check_disk().is_err() {
                Err(self.disk_conflict())
            } else {
                Err(EditorError::operation(error))
            };
        }
        let mut inputs = self.saved_inputs.clone();
        inputs
            .refresh_master_observation(&self.path)
            .map_err(EditorError::operation)?;
        if fs::read(&self.path).ok().as_deref() != Some(&*self.working) {
            return Err(self.disk_conflict());
        }
        self.saved = Arc::clone(&self.working);
        self.saved_inputs = inputs;
        self.watch.accepted(&self.saved_inputs);
        self.conflict = false;
        self.hud_version += 1;
        Ok(self.state())
    }

    fn reload(&mut self, discard: bool) -> Result<EditResult, EditorError> {
        let disk_changed = self.check_disk().is_err();
        if self.dirty() && !discard && disk_changed {
            return Err(self.disk_conflict());
        }
        if !disk_changed && !(discard && self.dirty()) {
            return Ok(self.result(None));
        }
        let mut install = || -> anyhow::Result<_> {
            let bytes: Master = fs::read(&self.path)?.into();
            let (change, next) = self.prepare(&bytes)?;
            next.snapshot.inputs.check()?;
            ensure!(
                &*bytes == fs::read(&self.path)?,
                "master changed while reloading"
            );
            let inputs = next.snapshot.inputs.clone();
            let transition = self.activate(next, change, true)?;
            Ok((bytes, inputs, transition))
        };
        let (bytes, inputs, transition) = install().map_err(|error| {
            self.watch.rejected(&error);
            EditorError::operation(error)
        })?;
        self.saved = Arc::clone(&bytes);
        self.working = bytes;
        self.saved_inputs = inputs;
        self.watch.accepted(&self.saved_inputs);
        self.undo.clear();
        self.redo.clear();
        self.conflict = false;
        Ok(self.result(Some(transition)))
    }

    pub fn poll_changes(&mut self) -> Option<Result<Value, EditorError>> {
        self.watch
            .take_changed()
            .then(|| self.reload(false).and_then(encode))
    }

    /// Sample the working edit using the accepted native renderer.
    pub fn render(&mut self, revision: u64, frame: u32) -> Result<Frame, EditorError> {
        self.check_revision(revision)?;
        self.frame_in_project(frame)?;
        self.viewer.seek(frame).map_err(EditorError::operation)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(picture) = self.viewer.poll().map_err(EditorError::operation)? {
                return Ok(picture);
            }
            if Instant::now() >= deadline {
                return Err(EditorError::new(
                    "operationFailed",
                    "timed out waiting for the editor picture",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn dispatch(&mut self, command: EditorCommand) -> Result<Value, EditorError> {
        self.dispatch_with_picture(command)
            .map(|reply| reply.result)
    }

    /// An edit's already validated parked picture accompanies its state and change.
    pub fn dispatch_with_picture(
        &mut self,
        command: EditorCommand,
    ) -> Result<EditorReply, EditorError> {
        let mut transition = None;
        let result = self.dispatch_command(command, &mut transition)?;
        Ok(EditorReply { result, transition })
    }

    fn dispatch_command(
        &mut self,
        command: EditorCommand,
        transition: &mut Option<(Frame, RevisionChange)>,
    ) -> Result<Value, EditorError> {
        let mut finish_edit = |edit: EditResult| {
            let result = encode(&edit)?;
            *transition = edit.picture.zip(edit.change);
            Ok(result)
        };
        match command {
            EditorCommand::GetState => encode(self.state()),
            EditorCommand::Inspect => encode(self.inspection()),
            EditorCommand::Seek { frame } => {
                self.set_playhead(frame)?;
                encode(self.state())
            }
            EditorCommand::EnterScope { revision, scope } => {
                self.check_revision(revision)?;
                if scope != ROOT && self.inspection().placement(&scope).is_none() {
                    return Err(EditorError::new("invalidTarget", "unknown timeline scope"));
                }
                if scope != self.scope {
                    self.scope = scope;
                    self.selections.clear();
                    self.marquee = None;
                    self.hover = None;
                    self.hud_version += 1;
                }
                encode(self.state())
            }
            EditorCommand::HitTest {
                revision,
                frame,
                point,
            } => {
                self.check_revision(revision)?;
                encode(self.hit_test(frame, point)?)
            }
            EditorCommand::SelectAt {
                revision,
                frame,
                point,
            } => {
                self.check_revision(revision)?;
                self.select_at(frame, point)?;
                encode(self.state())
            }
            EditorCommand::HoverAt {
                revision,
                frame,
                point,
            } => {
                self.check_revision(revision)?;
                self.hover_at(frame, point)?;
                encode(self.state())
            }
            EditorCommand::ClearHover { revision } => {
                self.check_revision(revision)?;
                self.change_hover(None);
                encode(self.state())
            }
            EditorCommand::BeginMarquee {
                revision,
                frame,
                point,
                threshold,
            } => {
                self.check_revision(revision)?;
                self.begin_marquee(frame, point, threshold)?;
                encode(self.state())
            }
            EditorCommand::UpdateMarquee { revision, point } => {
                self.check_revision(revision)?;
                self.update_marquee(point)?;
                encode(self.state())
            }
            EditorCommand::FinishMarquee { revision, point } => {
                self.check_revision(revision)?;
                self.finish_marquee(point)?;
                encode(self.state())
            }
            EditorCommand::CancelMarquee { revision } => {
                self.check_revision(revision)?;
                self.cancel_marquee();
                encode(self.state())
            }
            EditorCommand::LayerGeometry {
                revision,
                frame,
                layer,
            } => {
                self.check_revision(revision)?;
                encode(self.geometry(frame, &layer)?)
            }
            EditorCommand::Select { revision, target } => {
                self.check_revision(revision)?;
                if !self.valid_target(&target) {
                    return Err(EditorError::new(
                        "invalidTarget",
                        "unknown selection target",
                    ));
                }
                self.change_selection(vec![target]);
                encode(self.state())
            }
            EditorCommand::ClearSelection => {
                self.change_selection(vec![]);
                encode(self.state())
            }
            EditorCommand::SetTrackEnabled {
                revision,
                scope,
                track,
                enabled,
            } => {
                self.check_revision(revision)?;
                finish_edit(self.toggle_v2(&scope, &track, enabled)?)
            }
            EditorCommand::Undo { revision } => {
                self.check_revision(revision)?;
                finish_edit(self.step_history(true)?)
            }
            EditorCommand::Redo { revision } => {
                self.check_revision(revision)?;
                finish_edit(self.step_history(false)?)
            }
            EditorCommand::Save { revision } => {
                self.check_revision(revision)?;
                encode(self.save()?)
            }
            EditorCommand::Reload {
                revision,
                discard_changes,
            } => {
                self.check_revision(revision)?;
                finish_edit(self.reload(discard_changes)?)
            }
            EditorCommand::Render {
                revision,
                frame,
                points,
            } => {
                let [width, height] = self.project().size;
                if points.len() > 64 || points.iter().any(|&[x, y]| x >= width || y >= height) {
                    return Err(EditorError::new(
                        "invalidPoint",
                        "request up to 64 in-bounds output pixels",
                    ));
                }
                let picture = self.render(revision, frame)?;
                let pixels = points
                    .iter()
                    .map(|&[x, y]| {
                        let offset = (y as usize * width as usize + x as usize) * 4;
                        &picture.rgba[offset..offset + 4]
                    })
                    .collect::<Vec<_>>();
                Ok(
                    json!({"revision": revision, "frame": frame, "size": picture.size, "pixels": pixels}),
                )
            }
        }
    }
}

fn encode(value: impl Serialize) -> Result<Value, EditorError> {
    serde_json::to_value(value).map_err(|error| EditorError::operation(error.into()))
}
