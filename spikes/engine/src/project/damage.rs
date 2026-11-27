//! Compare two validated authoring snapshots at the granularity of final picture and mix.
use super::{AuthoringDocument, FrameRate, Project, Shot};
use crate::{
    export::AudioClip,
    project_watch::{InputOwner, ProjectInputs},
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
mod timeline;
pub use timeline::{OccurrenceRanges, TimelineChange};

/// Impact of accepting an authoring revision, independent of the edit's origin or presenter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionChange {
    pub from_revision: u64,
    pub to_revision: u64,
    pub damage: ProjectDamage,
    pub timeline: TimelineChange,
}

impl RevisionChange {
    pub fn after(previous: u64, damage: ProjectDamage) -> Result<Self> {
        Ok(Self {
            from_revision: previous,
            to_revision: previous.checked_add(1).context("revision exhausted")?,
            damage,
            timeline: TimelineChange::Unknown,
        })
    }

    pub fn whole_project_after(previous: u64) -> Result<Self> {
        Self::after(previous, ProjectDamage::all())
    }
}

/// Potentially changed output over half-open project-frame ranges. Audio ranges
/// include the entire picture interval containing each affected sample.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OutputDamage {
    None,
    All,
    Ranges { ranges: Vec<[u32; 2]> },
}

impl OutputDamage {
    pub fn contains(&self, frame: u32) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Ranges { ranges } => {
                let i = ranges.partition_point(|&[_, end]| end <= frame);
                ranges.get(i).is_some_and(|&[start, _]| start <= frame)
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::None)
    }

    fn from_ranges(mut ranges: Vec<[u32; 2]>, duration: u32) -> Self {
        merge_ranges(&mut ranges);
        match ranges.as_slice() {
            [] => Self::None,
            [[0, end]] if *end == duration => Self::All,
            _ => Self::Ranges { ranges },
        }
    }
}

fn merge_ranges(ranges: &mut Vec<[u32; 2]>) {
    ranges.sort_unstable();
    ranges.dedup_by(|next, previous| {
        if next[0] <= previous[1] {
            previous[1] = previous[1].max(next[1]);
            true
        } else {
            false
        }
    });
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectDamage {
    pub picture: OutputDamage,
    pub audio: OutputDamage,
}

impl ProjectDamage {
    pub fn all() -> Self {
        Self {
            picture: OutputDamage::All,
            audio: OutputDamage::All,
        }
    }
    pub fn between(
        previous: &AuthoringDocument,
        previous_inputs: &ProjectInputs,
        candidate: &AuthoringDocument,
        candidate_inputs: &ProjectInputs,
    ) -> Self {
        Self::classify(
            &previous.project,
            previous_inputs,
            &candidate.project,
            candidate_inputs,
        )
        .0
    }

    /// Keep the changed dependency owners so the timeline can explain this comparison.
    fn classify(
        old: &Project,
        previous_inputs: &ProjectInputs,
        new: &Project,
        candidate_inputs: &ProjectInputs,
    ) -> (Self, Option<BTreeSet<InputOwner>>) {
        let Ok(duration) = new.duration() else {
            return (Self::all(), None);
        };
        if old.fps != new.fps
            || old.duration().ok() != Some(duration)
            || !previous_inputs.observed()
            || !candidate_inputs.observed()
        {
            return (Self::all(), None);
        }

        let mut changed = previous_inputs.changed_resources(candidate_inputs);
        changed.extend(candidate_inputs.changed_resources(previous_inputs));
        let damage = Self {
            picture: picture_damage(old, new, &changed, duration),
            audio: audio_damage(old, new, &changed, duration),
        };
        (damage, Some(changed))
    }
}

impl RevisionChange {
    /// Each manifest must be the one captured while loading its document.
    pub fn between(
        revision: u64,
        previous: &AuthoringDocument,
        previous_inputs: &ProjectInputs,
        candidate: &AuthoringDocument,
        candidate_inputs: &ProjectInputs,
    ) -> Result<Self> {
        let (damage, resources) = ProjectDamage::classify(
            &previous.project,
            previous_inputs,
            &candidate.project,
            candidate_inputs,
        );
        let mut change = Self::after(revision, damage)?;
        if let Some(changed) = resources {
            change.timeline = timeline::compare(previous, candidate, &changed)?;
        }
        Ok(change)
    }
}

fn picture_damage(
    old: &Project,
    new: &Project,
    changed: &BTreeSet<InputOwner>,
    duration: u32,
) -> OutputDamage {
    let Project {
        size,
        title,
        graphics,
        exposure,
        shots,
        overlays,
        fps: _,
        audio: _,
    } = new;
    if old.size != *size
        || old.title != *title
        || old.graphics != *graphics
        || old.exposure != *exposure
        || changed.contains(&InputOwner::ProjectPicture)
        || old.shots.len() != shots.len()
        || old.overlays.len() != overlays.len()
    {
        return OutputDamage::All;
    }

    let mut ranges = Vec::new();
    let (mut old_start, mut new_start) = (0, 0);
    for (index, (before, after)) in old.shots.iter().zip(shots).enumerate() {
        let old_end = old_start + before.duration;
        let new_end = new_start + after.duration;
        if old_start != new_start || old_end != new_end
            || changed.contains(&InputOwner::Shot(index))
            // The USD resolver may consult layers and textures outside the direct manifest.
            || crate::camera::contains_scene3d(&after.graphics)
            || !same_shot_artwork(before, after)
        {
            ranges.extend([[old_start, old_end], [new_start, new_end]]);
        } else if before.camera_cuts != after.camera_cuts {
            compare_cuts(before, after, new_start, &mut ranges);
        }
        old_start = old_end;
        new_start = new_end;
    }
    for (index, (before, after)) in old.overlays.iter().zip(overlays).enumerate() {
        if (before.enabled || after.enabled)
            && (before != after || changed.contains(&InputOwner::Overlay(index)))
        {
            ranges.extend([
                [before.start, before.start + before.duration],
                [after.start, after.start + after.duration],
            ]);
        }
    }
    OutputDamage::from_ranges(ranges, duration)
}

fn same_shot_artwork(before: &Shot, after: &Shot) -> bool {
    let Shot {
        cameras,
        camera,
        camera_cuts: _,
        name: _,
        source,
        source_in,
        duration: _,
        background,
        graphics,
    } = after;
    before.cameras == *cameras
        && before.camera == *camera
        && before.source == *source
        && before.source_in == *source_in
        && before.background == *background
        && before.graphics == *graphics
}

fn compare_cuts(before: &Shot, after: &Shot, origin: u32, ranges: &mut Vec<[u32; 2]>) {
    // A validated cut list tiles its shot. Selection is constant between any
    // two cut ends from the two revisions, even if the selected camera is animated.
    let mut boundaries = vec![0, after.duration];
    boundaries.extend(
        before
            .camera_cuts
            .iter()
            .chain(&after.camera_cuts)
            .map(|cut| cut.end),
    );
    boundaries.sort_unstable();
    boundaries.dedup();
    for ends in boundaries.windows(2) {
        let start = ends[0];
        if before.camera_at(start) != after.camera_at(start) {
            ranges.push([origin + start, origin + ends[1]]);
        }
    }
}

fn audio_damage(
    old: &Project,
    new: &Project,
    changed: &BTreeSet<InputOwner>,
    duration: u32,
) -> OutputDamage {
    let mut ranges = Vec::new();
    for index in 0..old.audio.len().max(new.audio.len()) {
        let before = old.audio.get(index);
        let after = new.audio.get(index);
        if before != after || changed.contains(&InputOwner::Audio(index)) {
            ranges.extend(
                before
                    .into_iter()
                    .chain(after)
                    .map(|clip| audio_span(clip, new.fps, duration)),
            );
        }
    }
    OutputDamage::from_ranges(ranges, duration)
}

fn audio_span(clip: &AudioClip, fps: FrameRate, duration: u32) -> [u32; 2] {
    let first = clip.start_sample;
    let last = first + (clip.samples - 1);
    [
        fps.frame_at_sample(first, duration),
        fps.frame_at_sample(last, duration) + 1,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    type Snapshot = (AuthoringDocument, ProjectInputs);

    fn compare(a: &Snapshot, b: &Snapshot) -> ProjectDamage {
        ProjectDamage::between(&a.0, &a.1, &b.0, &b.1)
    }

    fn ranges(spans: &[[u32; 2]]) -> OutputDamage {
        OutputDamage::Ranges {
            ranges: spans.to_vec(),
        }
    }

    fn timeline(a: &Snapshot, b: &Snapshot) -> TimelineChange {
        RevisionChange::between(0, &a.0, &a.1, &b.0, &b.1)
            .unwrap()
            .timeline
    }

    fn entry(path: &str, ranges: &[[u32; 2]]) -> OccurrenceRanges {
        OccurrenceRanges {
            path: path.into(),
            ranges: ranges.into(),
        }
    }

    #[test]
    fn nested_shared_detail_and_same_path_media_edits_preserve_gaps() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        fs::create_dir(root.join("sequences"))?;
        let master = root.join("project.proj");
        let title = root.join("sequences/title.xml");
        let blank = r##"<sequence background="#ffffff"/>"##;
        fs::write(root.join("sequences/blank.xml"), blank)?;
        fs::write(
            &title,
            r##"<sequence background="#000000"><g><image source="poster.png"/></g></sequence>"##,
        )?;
        fs::write(root.join("poster.png"), "old")?;
        fs::write(
            &master,
            "project test fps 30/1 size 64x36\n[0,5) V1 blank\n[5,10) V1 title\n[10,21) V1 nested\n  [0,6) V1 blank\n  [6,11) V1 title\n[21,26) V1 blank\n",
        )?;
        let a = ProjectInputs::load(root)?;
        fs::write(root.join("sequences/blank.xml"), format!("  {blank}\n"))?;
        let formatted = ProjectInputs::load(root)?;
        assert_eq!(
            timeline(&a, &formatted),
            TimelineChange::Known {
                occurrences: vec![],
                thumbnails: vec![]
            }
        );
        assert_eq!(
            compare(&a, &formatted),
            ProjectDamage {
                picture: OutputDamage::None,
                audio: OutputDamage::None
            }
        );

        let sparse = ProjectDamage {
            picture: ranges(&[[5, 10], [16, 21]]),
            audio: OutputDamage::None,
        };
        fs::write(
            &title,
            fs::read_to_string(&title)?.replace("#000000", "#003355"),
        )?;
        let painted = ProjectInputs::load(root)?;
        assert_eq!(compare(&formatted, &painted), sparse);
        let outer = entry("V1:title@5", &[[5, 10]]);
        let inner = entry("V1:nested@10/V1:title@6", &[[16, 21]]);
        let changed = TimelineChange::Known {
            occurrences: vec![inner.clone(), outer.clone()],
            thumbnails: vec![entry("V1:nested@10", &[[16, 21]]), inner, outer],
        };
        assert_eq!(timeline(&formatted, &painted), changed);
        fs::write(root.join("new-image"), "new pixels")?;
        fs::rename(root.join("new-image"), root.join("poster.png"))?;
        let replaced = ProjectInputs::load(root)?;
        assert_eq!(compare(&painted, &replaced), sparse);
        assert_eq!(timeline(&painted, &replaced), changed);
        assert!(!sparse.picture.contains(10));
        assert!(sparse.picture.contains(16));
        Ok(())
    }

    #[test]
    fn gain_and_shared_source_have_independent_output_ranges() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir(root.join("sequences"))?;
        fs::write(root.join("media.mov"), "one")?;
        fs::write(
            root.join("project.proj"),
            "project test fps 30000/1001 size 64x36\n[0,4) V1 video\n[1,3) A1 mix\n[4,10) V1 blank\n[7,9) A1 mix\n",
        )?;
        fs::write(
            root.join("sequences/video.json"),
            r#"{"source":"media.mov"}"#,
        )?;
        fs::write(
            root.join("sequences/blank.xml"),
            r##"<sequence background="#ffffff"/>"##,
        )?;
        let score = root.join("sequences/mix.json");
        let write_gain = |gain| {
            fs::write(
                &score,
                format!(r#"{{"source":"media.mov","source_in_sample":0,"gain":{gain}}}"#),
            )
        };
        write_gain(1.0)?;
        let original = ProjectInputs::load(root)?;
        write_gain(0.5)?;
        let gain = ProjectInputs::load(root)?;
        let mix_ranges = ranges(&[[1, 3], [7, 9]]);
        assert_eq!(
            timeline(&original, &gain),
            TimelineChange::Known {
                occurrences: vec![entry("A1:mix@1", &[[1, 3]]), entry("A1:mix@7", &[[7, 9]])],
                thumbnails: vec![],
            }
        );
        assert_eq!(
            compare(&original, &gain),
            ProjectDamage {
                picture: OutputDamage::None,
                audio: mix_ranges.clone()
            }
        );

        fs::write(root.join("media.mov"), "new AV")?;
        let media = ProjectInputs::load(root)?;
        assert_eq!(
            compare(&gain, &media),
            ProjectDamage {
                picture: ranges(&[[0, 4]]),
                audio: mix_ranges
            }
        );
        Ok(())
    }

    #[test]
    fn unaligned_samples_round_outward_at_rational_frame_boundaries() -> Result<()> {
        let rate = FrameRate(30_000, 1001);
        let boundary = crate::export::sample_boundary(rate, 257)?;
        let mut clip = AudioClip {
            source: "score".into(),
            start_sample: boundary - 1,
            source_in_sample: 0,
            samples: 2,
            gain: 1.0,
        };
        assert_eq!(audio_span(&clip, rate, 300), [256, 258]);
        clip.samples = 1;
        assert_eq!(audio_span(&clip, rate, 300), [256, 257]);
        clip.start_sample = boundary;
        assert_eq!(audio_span(&clip, rate, 300), [257, 258]);
        Ok(())
    }

    #[test]
    fn rolling_a_camera_cut_affects_only_its_changed_selection() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir(root.join("sequences"))?;
        fs::write(
            root.join("sequences/shot.xml"),
            r##"<sequence background="#ffffff" camera="wide"><camera id="wide" projection="orthographic"/><camera id="close" projection="orthographic" zoom="2"/></sequence>"##,
        )?;
        let write_cut = |cut| {
            fs::write(
                root.join("project.proj"),
                format!(
                    "project test fps 24/1 size 64x36\n[0,10) V1 shot\n[10,20) V1 shot\n  [0,{cut}) CAM wide\n  [{cut},10) CAM close\n"
                ),
            )
        };
        write_cut(2)?;
        let initial = ProjectInputs::load(root)?;
        write_cut(3)?;
        let rolled = ProjectInputs::load(root)?;
        let changed = timeline(&initial, &rolled);
        let second = entry("V1:shot@10", &[[12, 13]]);
        assert_eq!(
            changed,
            TimelineChange::Known {
                occurrences: vec![second.clone()],
                thumbnails: vec![second]
            }
        );
        assert!(changed.preserves_thumbnail("V1:shot@10", 11));
        assert!(!changed.preserves_thumbnail("V1:shot@10", 12));
        assert!(changed.preserves_thumbnail("V1:shot@10", 13));
        assert_eq!(
            compare(&initial, &rolled),
            ProjectDamage {
                picture: ranges(&[[12, 13]]),
                audio: OutputDamage::None
            }
        );
        Ok(())
    }

    #[test]
    fn moving_an_overlay_touches_old_and_new_spans_and_clock_changes_drop_everything() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir(root.join("sequences"))?;
        fs::write(
            root.join("sequences/bed.xml"),
            r##"<sequence background="#ffffff"/>"##,
        )?;
        fs::write(
            root.join("sequences/card.xml"),
            r##"<sequence><rect width="10" height="5" fill="#001122"/></sequence>"##,
        )?;
        let master = root.join("project.proj");
        let initial = "project test fps 30/1 size 64x36\n[0,20) V1 bed\n[2,5) V2 card\n";
        fs::write(&master, initial)?;
        let old = ProjectInputs::load(root)?;
        let moved = initial.replace("[2,5)", "[9,12)");
        fs::write(&master, &moved)?;
        let new = ProjectInputs::load(root)?;
        assert_eq!(timeline(&old, &new), TimelineChange::Unknown);
        assert!(!timeline(&old, &new).preserves_thumbnail("V1:bed@0", 12));
        assert_eq!(
            compare(&old, &new),
            ProjectDamage {
                picture: ranges(&[[2, 5], [9, 12]]),
                audio: OutputDamage::None
            }
        );
        fs::write(&master, moved.replace("30/1", "24/1"))?;
        assert_eq!(
            compare(&new, &ProjectInputs::load(root)?),
            ProjectDamage::all()
        );
        Ok(())
    }

    #[test]
    fn a_spatial_stage_requires_new_pixels_even_if_direct_files_match() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/field-unit");
        let baseline = ProjectInputs::load(&root)?;
        let reopened = ProjectInputs::load(&root)?;
        let TimelineChange::Known {
            occurrences,
            thumbnails,
        } = timeline(&baseline, &reopened)
        else {
            panic!("expected the spatial placement to retain its identity");
        };
        assert!(occurrences.is_empty());
        assert!(!thumbnails.is_empty());
        assert_eq!(
            compare(&baseline, &reopened),
            ProjectDamage {
                picture: OutputDamage::All,
                audio: OutputDamage::None
            }
        );
        Ok(())
    }

    #[test]
    fn overlay_edit_invalidates_only_its_own_and_ancestor_thumbnails() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir(root.join("sequences"))?;
        fs::write(
            root.join("project.proj"),
            "project test fps 30/1 size 64x36\n[0,20) V1 parent\n  [0,20) V1 plate\n  [2,12) V2 card\n[0,20) V2 bug\n",
        )?;
        fs::write(
            root.join("sequences/plate.xml"),
            r##"<sequence background="#ffffff"/>"##,
        )?;
        let card = root.join("sequences/card.xml");
        let art = r##"<sequence><rect width="10" height="5" fill="#112233"/></sequence>"##;
        fs::write(&card, art)?;
        fs::write(root.join("sequences/bug.xml"), art)?;
        let old = ProjectInputs::load(root)?;
        fs::write(&card, art.replace("#112233", "#445566"))?;
        let new = ProjectInputs::load(root)?;
        let leaf = entry("V1:parent@0/V2:card@2", &[[2, 12]]);
        let result = timeline(&old, &new);
        assert_eq!(
            result,
            TimelineChange::Known {
                occurrences: vec![leaf.clone()],
                thumbnails: vec![entry("V1:parent@0", &[[2, 12]]), leaf],
            }
        );
        assert!(result.preserves_thumbnail("V2:bug@0", 2));
        assert!(result.preserves_thumbnail("V1:parent@0/V1:plate@0", 2));
        Ok(())
    }

    #[test]
    fn root_v2_toggle_preserves_nested_tracks_and_isolated_thumbnails() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let master = root.join("project.proj");
        let sequences = root.join("sequences");
        fs::create_dir(&sequences)?;
        fs::write(
            sequences.join("plate.xml"),
            r##"<sequence background="#ffffff"/>"##,
        )?;
        fs::write(
            sequences.join("card.xml"),
            r##"<sequence><rect width="10" height="5" fill="#112233"/></sequence>"##,
        )?;
        let original = concat!(
            "project test fps 30/1 size 64x36  # heading\r\n",
            "# master note\r\n",
            "[0,5) V1 plate\r\n",
            "[2,5) V2 card\r\n",
            "[5,15) V1 nested\r\n",
            "  [0,10) V1 plate\r\n",
            "  [0,10) V2 card\r\n",
            "[15,20) V1 plate\r\n",
            "[15,18) V2 card\r\n",
        );
        fs::write(&master, original)?;
        let old = ProjectInputs::load(root)?;
        let off = crate::project::set_v2_enabled(original.as_bytes(), false)?;
        let expected = original.replace("# master note", "track V2 off\r\n# master note");
        assert_eq!(off, expected.as_bytes());
        assert_eq!(fs::read(&master)?, original.as_bytes());
        let decorated = b"project sample fps 24/1 size 10x10\r\ntrack V2\t off  # note\r\n";
        assert_eq!(
            crate::project::set_v2_enabled(decorated, true)?,
            b"project sample fps 24/1 size 10x10\r\ntrack V2\t on  # note\r\n"
        );
        let mut new = ProjectInputs::load_with_master(root, &off)?;
        new.1.check()?;
        assert_eq!(new.0.root_v2_enabled(), Some(false));
        let inspection = new.0.inspection()?;
        assert!(!inspection.placement("V2:card@2").unwrap().enabled);
        assert!(
            inspection
                .placement("V1:nested@5/V2:card@0")
                .unwrap()
                .enabled
        );
        assert_eq!(old.0.project.active_overlays(3).count(), 1);
        assert_eq!(new.0.project.active_overlays(3).count(), 0);
        assert_eq!(new.0.project.active_overlays(11).count(), 1);
        assert_eq!(
            compare(&old, &new),
            ProjectDamage {
                picture: ranges(&[[2, 5], [15, 18]]),
                audio: OutputDamage::None,
            }
        );
        assert_eq!(
            timeline(&old, &new),
            TimelineChange::Known {
                occurrences: vec![
                    entry("V2:card@15", &[[15, 18]]),
                    entry("V2:card@2", &[[2, 5]])
                ],
                thumbnails: vec![],
            }
        );
        fs::write(&master, &off)?;
        assert!(new.1.check().is_err());
        new.1.refresh_master_observation(root)?;
        new.1.check()?;
        let saved = ProjectInputs::load(root)?;
        assert_eq!(saved.0.project, new.0.project);
        Ok(())
    }
}
