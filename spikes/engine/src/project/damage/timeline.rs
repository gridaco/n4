use super::{compare_cuts, merge_ranges, same_shot_artwork};
use crate::{
    AuthoringDocument, Project,
    inspect::{Inspection, Layout, Placement},
    project_watch::InputOwner,
};
use anyhow::Result;
use serde::Serialize;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

/// A path in the accepted inspection and half-open intervals in project time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OccurrenceRanges {
    pub path: String,
    pub ranges: Vec<[u32; 2]>,
}

/// With a known transition, an omitted path that survives in both inspections is
/// clean. Occurrences identify authored edits; thumbnails identify affected samples.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TimelineChange {
    Known {
        occurrences: Vec<OccurrenceRanges>,
        thumbnails: Vec<OccurrenceRanges>,
    },
    Unknown,
}

impl TimelineChange {
    /// Known transitions certify the sample only if its occurrence survives.
    /// The evaluator returns Unknown whenever it cannot match the two layouts.
    pub fn preserves_thumbnail(&self, path: &str, frame: u32) -> bool {
        let Self::Known { thumbnails, .. } = self else {
            return false;
        };
        match thumbnails.binary_search_by_key(&path, |record| record.path.as_str()) {
            Ok(i) => !thumbnails[i]
                .ranges
                .iter()
                .any(|span| (span[0]..span[1]).contains(&frame)),
            Err(_) => true,
        }
    }
}

type RangesByPath = BTreeMap<String, Vec<[u32; 2]>>;

fn placements(doc: &AuthoringDocument) -> Result<Cow<'_, [Placement]>> {
    match &doc.layout {
        Layout::Master { placements, .. } => Ok(Cow::Borrowed(placements)),
        Layout::Single(file) => Ok(Cow::Owned(
            Inspection::single(&doc.project, file)?.placements,
        )),
    }
}

pub(super) fn compare(
    old: &AuthoringDocument,
    new: &AuthoringDocument,
    resources: &BTreeSet<InputOwner>,
) -> Result<TimelineChange> {
    let a = &old.project;
    let b = &new.project;
    if a.size != b.size
        || a.exposure != b.exposure
        || a.graphics != b.graphics
        || a.title != b.title
        || resources.contains(&InputOwner::ProjectPicture)
    {
        return Ok(TimelineChange::Unknown);
    }
    let mut changes = Changes {
        old: a,
        new: b,
        resources,
        occurrences: BTreeMap::new(),
        thumbnails: BTreeMap::new(),
    };
    if !changes.visit(&placements(old)?, &placements(new)?) {
        return Ok(TimelineChange::Unknown);
    }
    Ok(TimelineChange::Known {
        occurrences: records(changes.occurrences),
        thumbnails: records(changes.thumbnails),
    })
}

fn records(map: RangesByPath) -> Vec<OccurrenceRanges> {
    map.into_iter()
        .map(|(path, mut ranges)| {
            merge_ranges(&mut ranges);
            OccurrenceRanges { path, ranges }
        })
        .collect()
}

struct Changes<'a> {
    old: &'a Project,
    new: &'a Project,
    resources: &'a BTreeSet<InputOwner>,
    occurrences: RangesByPath,
    thumbnails: RangesByPath,
}

impl Changes<'_> {
    fn visit(&mut self, old: &[Placement], new: &[Placement]) -> bool {
        // CAM rows can move while their owning V1 retains its identity. The shot
        // comparison below can then report precisely where camera selection differs.
        let mut old = old.iter().filter(|p| p.track != "CAM");
        for b in new.iter().filter(|p| p.track != "CAM") {
            let Some(a) = old.next() else {
                return false;
            };
            if a.path != b.path
                || a.project != b.project
                || a.owner != b.owner
                || a.definition != b.definition
            {
                return false;
            }
            self.compare_placement(a, b);
            if !self.visit(&a.children, &b.children) {
                return false;
            }
        }
        old.next().is_none()
    }

    fn compare_placement(&mut self, a: &Placement, b: &Placement) {
        let mut authored = Vec::new();
        let mut thumbnail = Vec::new();
        match b.owner {
            Some(owner @ InputOwner::Shot(index)) => {
                let before = &self.old.shots[index];
                let after = &self.new.shots[index];
                if self.resources.contains(&owner) || !same_shot_artwork(before, after) {
                    authored.push(b.project);
                } else if before.camera_cuts != after.camera_cuts {
                    compare_cuts(before, after, b.project[0], &mut authored);
                }
                thumbnail.extend_from_slice(&authored);
                // Until the OpenUSD resolver can supply all transitive dependencies,
                // its sampled image remains uncertain even on a manual reopen.
                if crate::camera::contains_scene3d(&after.graphics) {
                    thumbnail.push(b.project);
                }
                if before.name != after.name {
                    authored.push(b.project);
                }
            }
            Some(owner @ InputOwner::Overlay(index)) => {
                let before = &self.old.overlays[index];
                let after = &self.new.overlays[index];
                let artwork_changed =
                    before.graphics != after.graphics || self.resources.contains(&owner);
                if artwork_changed || before.enabled != after.enabled {
                    authored.push(b.project);
                }
                if artwork_changed {
                    thumbnail.push(b.project);
                }
                if before.enabled != after.enabled
                    && let Some((parent, _)) = b.path.rsplit_once('/')
                {
                    // Visibility changes the composite of an ancestor. The
                    // placement's own tile still samples its artwork.
                    self.add_thumbnail_ranges(parent, &[b.project]);
                }
            }
            Some(owner @ InputOwner::Audio(index)) => {
                if self.resources.contains(&owner) || self.old.audio[index] != self.new.audio[index]
                {
                    authored.push(b.project);
                }
            }
            _ => {}
        }
        if a.name != b.name {
            authored.push(b.project);
        }
        if !authored.is_empty() {
            self.occurrences.insert(b.path.clone(), authored);
        }
        if !thumbnail.is_empty() {
            // ClipThumbnails renders a V1 together with its descendants. All
            // editorial ancestors in the master are V1 compositions. A sibling
            // overlay samples its own neutral-backed view and stays clean.
            self.add_thumbnail_ranges(&b.path, &thumbnail);
        }
    }

    fn add_thumbnail_ranges(&mut self, mut path: &str, ranges: &[[u32; 2]]) {
        loop {
            self.thumbnails
                .entry(path.into())
                .or_default()
                .extend_from_slice(ranges);
            match path.rsplit_once('/') {
                Some((parent, _)) => path = parent,
                None => break,
            }
        }
    }
}
