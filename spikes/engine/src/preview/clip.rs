//! Sample an individual timeline occurrence on the sidecar thumbnail worker.
use super::{Latest, Thumbnail, resample};
use crate::{Background, Engine, Project, Shot, inspect::Placement, media};
use anyhow::{Context, Result, bail, ensure};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MEDIA_PROFILES: usize = 8;
const THUMBNAIL_SIZE: u32 = 120;

pub(crate) struct ClipFrame {
    frame: u32,
    scope: Scope,
}

enum Scope {
    Picture(String),
    Overlay(String),
}

pub(crate) type ClipThumbnails = Latest<Thumbnail, ClipFrame>;

impl ClipThumbnails {
    pub(crate) fn for_project(
        project: Project,
        placements: &[Placement],
        svg_assets: Option<Arc<crate::graphics::PreparedSvgAssets>>,
        text_layouts: Option<Arc<crate::graphics::PreparedTextLayouts>>,
    ) -> Result<Self> {
        let [width, height] = project.size;
        let longest = width.max(height);
        let size = project
            .size
            .map(|side| ((side * THUMBNAIL_SIZE + longest / 2) / longest).max(1));
        let mut overlays = vec![String::new(); project.overlays.len()];
        overlay_paths(placements, &mut overlays);
        Self::spawn("n4-clip-thumbnails", move || {
            let mut compositor = None;
            let mut media = MediaThumbnails::default();
            Ok(move |request: ClipFrame| {
                let frame = request.frame;
                let scope = &request.scope;
                let (shot, local) = project.shot_at(frame)?;
                if let Scope::Picture(path) = scope
                    && let Some(source) = shot.source.as_deref()
                    && shot.camera_at(local).is_none()
                    && shot.graphics.is_empty()
                    && contained_overlays(&project, &overlays, path, frame)
                        .next()
                        .is_none()
                {
                    return media.read(source, size, project.fps, shot.source_in + local);
                }
                let engine = match &mut compositor {
                    Some(engine) => engine,
                    None => {
                        let mut engine = Engine::new_with_artwork(
                            project.clone(),
                            svg_assets.as_deref(),
                            text_layouts.as_deref(),
                        )?;
                        engine.prepare_session_artwork()?;
                        compositor.insert(engine)
                    }
                };
                let (view, source_frame) =
                    scope.project_view(engine.project(), &overlays, frame)?;
                let picture = engine.render_view(frame, Some(&view))?;
                Ok(Thumbnail {
                    size,
                    rgba: Arc::new(resample(picture, size)?),
                    source: None,
                    source_frame,
                    proxy: None,
                })
            })
        })
    }

    pub(crate) fn request_placement(&self, frame: u32, placement: &Placement) -> Result<u64> {
        let [start, end] = placement.project;
        ensure!(
            (start..end).contains(&frame),
            "thumbnail frame is outside this placement"
        );
        let scope = match placement.track.as_str() {
            "V1" => Scope::Picture(placement.path.clone()),
            "V2" => Scope::Overlay(placement.path.clone()),
            _ => bail!("only picture placements have thumbnails"),
        };
        Ok(self.submit(ClipFrame { frame, scope }))
    }
}

// Inspection order can differ from paint order. Use the loader's occurrence mapping.
fn overlay_paths(placements: &[Placement], paths: &mut [String]) {
    for placement in placements {
        if let Some(crate::project_watch::InputOwner::Overlay(index)) = placement.owner {
            paths[index].clone_from(&placement.path);
        }
        overlay_paths(&placement.children, paths);
    }
}

fn contained_overlays<'a>(
    project: &'a Project,
    overlays: &'a [String],
    parent: &'a str,
    frame: u32,
) -> impl Iterator<Item = &'a crate::GraphicsPlacement> {
    overlays
        .iter()
        .zip(&project.overlays)
        .filter_map(move |(path, overlay)| {
            let descendant = path
                .strip_prefix(parent)
                .is_some_and(|suffix| suffix.starts_with('/'));
            (descendant && overlay.enabled && overlay.local_frame(frame).is_some())
                .then_some(overlay)
        })
}

impl Scope {
    fn project_view(
        &self,
        project: &Project,
        overlays: &[String],
        frame: u32,
    ) -> Result<(Project, u32)> {
        let mut view = Project {
            size: project.size,
            fps: project.fps,
            shots: vec![],
            overlays: vec![],
            title: Default::default(),
            audio: vec![],
            graphics: vec![],
            exposure: project.exposure,
        };
        let local = match self {
            Self::Picture(path) => {
                let (shot, local) = project.shot_at(frame)?;
                // Keep the shot and the contained overlays on their original clocks.
                if frame > local {
                    view.shots.push(neutral(frame - local));
                }
                view.shots.push(shot.clone());
                view.overlays = contained_overlays(project, overlays, path, frame)
                    .cloned()
                    .collect();
                local
            }
            Self::Overlay(path) => {
                view.shots.push(neutral(project.duration()?));
                if let Some(index) = overlays.iter().position(|p| p == path) {
                    let overlay = &project.overlays[index];
                    let local = overlay
                        .local_frame(frame)
                        .context("graphics thumbnail outside placement")?;
                    let mut isolated = overlay.clone();
                    isolated.enabled = true;
                    view.overlays.push(isolated);
                    local
                } else if path == "V2:graphics@0" {
                    view.graphics = project.graphics.clone();
                    frame
                } else if path.starts_with("V2:title@") {
                    view.title = project.title.clone();
                    frame.saturating_sub(project.title.start)
                } else {
                    bail!("missing resolved graphics occurrence {path}");
                }
            }
        };
        Ok((view, local))
    }
}

fn neutral(duration: u32) -> Shot {
    Shot {
        cameras: vec![],
        camera: None,
        camera_cuts: vec![],
        name: None,
        source: None,
        source_in: 0,
        duration,
        background: Some(Background {
            color: [24, 24, 27],
            grain: 0,
            seed: 0,
        }),
        graphics: vec![],
    }
}

struct MediaThumbnails {
    reader: media::VideoReader,
    profiles: VecDeque<(PathBuf, media::IndexedVideo)>,
}

impl Default for MediaThumbnails {
    fn default() -> Self {
        Self {
            reader: media::VideoReader::with_limits(16 * 1024 * 1024, 4 * 1024 * 1024),
            profiles: VecDeque::new(),
        }
    }
}

impl MediaThumbnails {
    fn read(
        &mut self,
        path: &Path,
        [width, height]: [u32; 2],
        fps: crate::FrameRate,
        tick: u32,
    ) -> Result<Thumbnail> {
        let profile =
            if let Some(index) = self.profiles.iter().position(|(source, _)| source == path) {
                self.profiles.remove(index).expect("known profile")
            } else {
                let index = media::IndexedVideo::open(path, width, height, fps)?;
                if self.profiles.len() == MEDIA_PROFILES {
                    self.profiles.pop_front();
                    self.reader.retain_sources(|key| {
                        self.profiles
                            .iter()
                            .any(|(_, index)| index.has_identity(key))
                    });
                }
                (path.to_owned(), index)
            };
        self.profiles.push_back(profile);
        let (_, index) = self.profiles.back().expect("installed profile");
        self.reader.begin_frame([index]);
        let ordinal = index.resolve(tick)?;
        let (rgba, _) = self.reader.read(index, ordinal)?;
        Ok(Thumbnail {
            size: [width, height],
            rgba,
            source: Some(path.to_owned()),
            source_frame: ordinal,
            proxy: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn single_file_thumbnail_owners_follow_paint_order_even_when_sorted_for_inspection()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("film.json");
        fs::write(
            &path,
            r#"{
            "size":[64,32],"fps":[24,1],"shots":[{"duration":10,"background":{"color":[0,0,0]}}],
            "overlays":[{"detail":"late","start":6,"duration":4,"graphics":[]},
                        {"detail":"early","start":1,"duration":2,"graphics":[]}]
        }"#,
        )?;
        let doc = crate::AuthoringDocument::load(&path, None)?;
        let inspection = doc.inspection()?;
        let mut paths = vec![String::new(); doc.project.overlays.len()];
        overlay_paths(&inspection.placements, &mut paths);
        assert_eq!(paths, ["V2:overlay-1@6", "V2:overlay-2@1"]);
        Ok(())
    }

    #[test]
    fn thumbnails_isolate_overlaps_and_preserve_nested_clocks() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path();
        let sequences = path.join("sequences");
        fs::create_dir(&sequences)?;
        fs::write(
            path.join("project.proj"),
            concat!(
                "project thumbnail fps 24/1 size 64x32\n",
                "track V2 off\n",
                "[0,2) V1 intro\n",
                "[2,6) V1 nested\n",
                "  [0,4) V1 blue\n",
                "  [0,4) V2 inner\n",
                "[2,6) V2 outer\n",
            ),
        )?;
        fs::write(
            sequences.join("intro.xml"),
            r##"<sequence background="#ffffff"/>"##,
        )?;
        fs::write(
            sequences.join("blue.xml"),
            r##"<sequence background="#0000ff"/>"##,
        )?;
        fs::write(
            sequences.join("inner.xml"),
            r##"<sequence><rect width="32" height="32" fill="#ff0000"/></sequence>"##,
        )?;
        fs::write(
            sequences.join("outer.xml"),
            r##"<sequence><rect width="64" height="32" fill="#00ff00"/></sequence>"##,
        )?;
        let document = crate::AuthoringDocument::load(path, None)?;
        let inspection = document.inspection()?;
        assert!(!inspection.placement("V2:outer@2").unwrap().enabled);
        let worker =
            ClipThumbnails::for_project(document.project, &inspection.placements, None, None)?;
        let sample = |path: &str| -> Result<Thumbnail> {
            let occurrence = inspection.placement(path).context("missing test clip")?;
            ensure!(
                worker.request_placement(6, occurrence).is_err(),
                "placement end is exclusive"
            );
            worker.wait(worker.request_placement(4, occurrence)?)
        };
        let colors = |image: Thumbnail| {
            let [w, h] = image.size;
            let pixel = |x| {
                let start = ((w * (h / 2) + x) * 4) as usize;
                <[u8; 3]>::try_from(&image.rgba[start..start + 3]).unwrap()
            };
            [pixel(w / 4), pixel(w * 3 / 4)]
        };
        let blue = [0, 0, 255];
        let red = [255, 0, 0];
        let green = [0, 255, 0];
        assert_eq!(colors(sample("V1:nested@2")?), [red, blue]);
        assert_eq!(colors(sample("V1:nested@2/V1:blue@0")?), [blue, blue]);
        assert_eq!(
            colors(sample("V1:nested@2/V2:inner@0")?),
            [red, [24, 24, 27]]
        );
        assert_eq!(colors(sample("V2:outer@2")?), [green, green]);
        Ok(())
    }
}
