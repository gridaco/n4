//! Immutable animated sources shared across the workers of one accepted edit.
use super::{IndexedVideo, animation::Animation};
use crate::{FrameRate, Project};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

// An asset can be used as both a full-frame picture and a small inset.
pub(crate) type SourceProfiles = HashMap<(PathBuf, [u32; 2]), Arc<IndexedVideo>>;

#[derive(Default, Clone)]
pub(crate) struct PreparedAnimations {
    // A recorded None identifies ordinary movie footage in this revision.
    by_path: HashMap<PathBuf, Option<Arc<Animation>>>,
}

impl PreparedAnimations {
    pub(crate) fn prepare<'a>(
        project: &Project,
        accepted: impl Fn(&Path) -> Option<&'a Self>,
    ) -> Result<Self> {
        let paths: BTreeSet<&Path> = project
            .shots
            .iter()
            .filter_map(|s| s.source.as_deref())
            .chain(
                project
                    .video_clips()
                    .into_iter()
                    .map(|clip| clip.source.as_path()),
            )
            .collect();
        let mut by_path = HashMap::new();
        for path in paths {
            let animation = match accepted(path).and_then(|snapshot| snapshot.by_path.get(path)) {
                Some(accepted) => accepted.clone(),
                None => Animation::open(path)
                    .with_context(|| format!("prepare timed source {}", path.display()))?,
            };
            by_path.insert(path.to_owned(), animation);
        }
        Ok(Self { by_path })
    }

    pub(crate) fn get(&self, path: &Path) -> Option<&Arc<Animation>> {
        self.by_path.get(path).and_then(Option::as_ref)
    }

    pub(crate) fn profile(
        &self,
        path: &Path,
        size: [u32; 2],
        fps: FrameRate,
    ) -> Result<IndexedVideo> {
        match self.get(path) {
            Some(animation) => {
                IndexedVideo::from_animation(path, size[0], size[1], fps, Arc::clone(animation))
            }
            None => IndexedVideo::open_movie(path, size[0], size[1], fps),
        }
    }

    pub(crate) fn profiles(
        &self,
        project: &Project,
        existing: &SourceProfiles,
        include_layers: bool,
    ) -> Result<SourceProfiles> {
        let mut profiles = SourceProfiles::new();
        let mut add = |path: &Path, size: [u32; 2], start: u32, duration: u32| -> Result<()> {
            use std::collections::hash_map::Entry;
            let index = match profiles.entry((path.to_owned(), size)) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => {
                    let index = if let Some(existing) = existing.get(e.key()) {
                        existing.clone()
                    } else {
                        Arc::new(
                            self.profile(path, size, project.fps)
                                .with_context(|| format!("open {}", path.display()))?,
                        )
                    };
                    e.insert(index)
                }
            };
            ensure!(
                start
                    .checked_add(duration)
                    .is_some_and(|end| end <= index.frame_count()),
                "trim exceeds available frames in {}",
                path.display()
            );
            Ok(())
        };
        for shot in &project.shots {
            if let Some(source) = &shot.source {
                add(source, project.size, shot.source_in, shot.duration)?;
            }
        }
        if include_layers {
            for layer in project.video_clips() {
                add(&layer.source, layer.size, layer.source_in, layer.duration)?;
            }
        }
        Ok(profiles)
    }
}
