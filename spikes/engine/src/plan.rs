//! Explicit requests in the fixture's constant-rate frame grid. No parser, decoder or GPU types.
use crate::graphics::Graphic;
use crate::project::{GraphicsPlacement, Project, Shot, collect_video_clips};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameRequest {
    pub timeline_frame: u32,
    /// Frame in this shot, starting at zero independently of any media trim.
    pub shot_frame: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    pub camera_selection: CameraSelection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    /// Resolved to the decoded presentation ordinal in a rendered video's trace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_frame: Option<u32>,
    /// Prior source ticks, newest first, resolved to presentation ordinals in render traces.
    /// Entries before the shot starts have no source picture.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_history: Vec<Option<u32>>,
    /// Independently timed active pictures, in layer paint order. The renderer
    /// resolves each project-rate source tick to its presentation-order ordinal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub video_layers: Vec<VideoFrameRequest>,
    /// Active V2 details in paint order. Each placement starts its clock at zero.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<OverlayFrameRequest>,
    /// Opaque video first, followed by the paragraph at this rect and coverage.
    pub text: String,
    pub font: PathBuf,
    pub font_size: u32,
    pub title_position: [i32; 2],
    pub title_width: u32,
    pub title_opacity: [u32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backplate: Option<crate::project::Backplate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraSelection {
    Identity,
    DetailDefault,
    PlacementCut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverlayFrameRequest {
    pub detail: String,
    pub local_frame: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VideoFrameRequest {
    pub source: PathBuf,
    pub source_frame: u32,
    pub size: [u32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<PathBuf>,
}

impl Project {
    pub(crate) fn active_overlays(
        &self,
        frame: u32,
    ) -> impl Iterator<Item = (&GraphicsPlacement, u32)> {
        self.overlays
            .iter()
            .filter(|p| p.enabled)
            .filter_map(move |p| Some((p, p.local_frame(frame)?)))
    }

    pub(crate) fn paint_scopes<'a>(
        &'a self,
        shot: &'a Shot,
        local: u32,
        frame: u32,
    ) -> impl Iterator<Item = (&'a [Graphic], u32)> {
        std::iter::once((shot.graphics.as_slice(), local))
            .chain(
                self.active_overlays(frame)
                    .map(|(p, age)| (p.graphics.as_slice(), age)),
            )
            .chain(std::iter::once((self.graphics.as_slice(), frame)))
    }

    /// Each shot starts its artwork clock at zero, regardless of source trim.
    pub(crate) fn shot_at(&self, frame: u32) -> Result<(&Shot, u32)> {
        let mut local = frame;
        for shot in &self.shots {
            if local < shot.duration {
                return Ok((shot, local));
            }
            local -= shot.duration;
        }
        bail!("frame {frame} is outside the project")
    }

    pub(crate) fn request(&self, frame: u32) -> Result<FrameRequest> {
        let (shot, shot_frame) = self.shot_at(frame)?;
        let title = &self.title;
        let opacity = match frame.checked_sub(title.start) {
            None => [0, 1],
            Some(age) if age < title.fade_frames => [age, title.fade_frames],
            Some(_) => [1, 1],
        };
        let mut video_layers = Vec::new();
        let mut clips = Vec::new();
        for (layers, clock) in self.paint_scopes(shot, shot_frame, frame) {
            collect_video_clips(layers, Some(clock), &mut clips);
            video_layers.extend(clips.drain(..).filter_map(|clip| {
                Some(VideoFrameRequest {
                    source_frame: clip.source_frame(clock)?,
                    source: clip.source.clone(),
                    size: clip.size,
                    proxy: None,
                })
            }));
        }
        Ok(FrameRequest {
            timeline_frame: frame,
            shot_frame,
            camera: shot.camera_at(shot_frame).map(|c| c.id().to_owned()),
            camera_selection: if !shot.camera_cuts.is_empty() {
                CameraSelection::PlacementCut
            } else if shot.camera.is_some() {
                CameraSelection::DetailDefault
            } else {
                CameraSelection::Identity
            },
            source: shot.source.clone(),
            source_frame: shot.source.as_ref().map(|_| shot.source_in + shot_frame),
            source_history: (1..=crate::graphics::history_depth(&shot.graphics, Some(shot_frame)))
                .map(|age| {
                    shot_frame
                        .checked_sub(age)
                        .map(|prior| shot.source_in + prior)
                })
                .collect(),
            video_layers,
            overlays: self
                .active_overlays(frame)
                .map(|(p, local_frame)| OverlayFrameRequest {
                    detail: p.detail.clone(),
                    local_frame,
                })
                .collect(),
            text: title.text.clone(),
            font: title.font.clone(),
            font_size: title.font_size,
            title_position: title.position,
            title_width: title.width,
            title_opacity: opacity,
            backplate: title.backplate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn source_history_uses_the_active_depth_and_stops_at_the_placement_cut() -> Result<()> {
        let footage = |frames| {
            json!({"content":{"type":"footage",
            "history":{"shader":"afterglow.wgsl","frames":frames}}})
        };
        let deeper = json!({"span":[2,4],"content":{"type":"group","children":[footage(4)]}});
        let mut project: Project = serde_json::from_value(json!({
            "size":[64,32], "fps":[24,1], "shots":[
                {"source":"v.mov","duration":2},
                {"source":"v.mov","source_in":50,"duration":5,
                 "graphics":[footage(1),deeper]}]
        }))?;
        project.validate()?;
        assert!(project.request(1)?.source_history.is_empty());
        assert_eq!(project.request(2)?.source_history, [None]);
        assert_eq!(
            project.request(4)?.source_history,
            [Some(51), Some(50), None, None]
        );
        assert_eq!(project.request(6)?.source_history, [Some(53)]);
        project.graphics = project.shots[1].graphics.clone();
        assert!(project.validate().is_err());
        project.graphics.clear();
        project.shots[1].graphics = serde_json::from_value(json!([footage(13)]))?;
        assert!(project.validate().is_err());
        project.shots[1].graphics = serde_json::from_value(json!([footage(0)]))?;
        assert!(project.validate().is_err());
        Ok(())
    }

    #[test]
    fn composition_shots_own_their_artwork_clock() -> Result<()> {
        let video = |source: &str, source_in, start| {
            json!({"content":{"type":"video", "source":source, "size":[24,16],
                              "source_in":source_in, "start":start, "duration":3}})
        };
        let edit: Project = serde_json::from_value(json!({
            "size":[24,16], "fps":[30,1],
            "shots":[
                {"source":"cut.mov","source_in":17,"duration":4,
                 "graphics":[video("inset.mov",10,1)]},
                {"name":"Feather","background":{"color":[21,28,31]},"duration":5,
                 "graphics":[{"span":[1,4],"content":{"type":"group","children":[video("inset.mov",20,1)]}}]}
            ],
            "graphics":[video("overlay.mov",30,4)]
        }))?;
        let mut project = edit.resolved(Path::new("assets"))?;
        assert_eq!(project.duration()?, 9);
        assert_eq!(project.video_clips().len(), 3);
        let layers = |project: &Project, f| -> Result<Vec<u32>> {
            Ok(project
                .request(f)?
                .video_layers
                .iter()
                .map(|r| r.source_frame)
                .collect())
        };
        let media = serde_json::to_value(project.request(1)?)?;
        assert_eq!(media["source_frame"], 18);
        assert_eq!(media["shot_frame"], 1);
        assert_eq!(layers(&project, 1)?, vec![10]);
        assert_eq!(layers(&project, 4)?, vec![30]);
        assert_eq!(layers(&project, 5)?, vec![20, 31]);
        assert_eq!(layers(&project, 8)?, Vec::<u32>::new());
        let selected = project.request(5)?;
        let trace = serde_json::to_value(&selected)?;
        assert_eq!(trace["shot_frame"], 1);
        assert!(trace.get("source").is_none() && trace.get("source_frame").is_none());
        assert_eq!(
            selected.video_layers[0].source,
            Path::new("assets/inset.mov")
        );
        assert!(serde_json::to_value(&project)?.get("title").is_none());
        project.shots.swap(0, 1);
        assert_eq!(layers(&project, 1)?, vec![20]);
        assert_eq!(layers(&project, 6)?, vec![10, 32]);
        assert!(project.shot_at(9).is_err());
        Ok(())
    }

    #[test]
    fn invalid_shot_bases_and_local_ranges_are_rejected() -> Result<()> {
        let fill = json!({"color":[1,2,3]});
        for shot in [
            json!({"duration":3}),
            json!({"duration":3,"source":"v.mov","background":fill}),
            json!({"duration":3,"source_in":1,"background":fill}),
            json!({"duration":3,"background":fill,"graphics":[
                {"span":[2,4],"content":{"type":"path","d":"M0 0H1V1Z","fill":[0,0,0,255]}}]}),
            json!({"duration":3,"background":fill,"graphics":[
                {"content":{"type":"video","source":"v.mov","source_in":0,"start":2,"duration":2,"size":[2,2]}}]}),
        ] {
            let doc = json!({"size":[24,16],"fps":[30,1],
                             "shots":[shot, {"duration":5,"background":fill}]});
            let project: Project = serde_json::from_value(doc)?;
            assert!(project.validate().is_err());
        }
        Ok(())
    }

    #[test]
    fn legacy_zero_source_trim_and_empty_editable_title_are_serialized() -> Result<()> {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/title.json"))?;
        value["shots"][0]["source_in"] = json!(0);
        value["title"]["text"] = json!("");
        let project: Project = serde_json::from_value(value.clone())?;
        let serialized = serde_json::to_value(project)?;
        assert_eq!(serialized.pointer("/shots/0/source_in"), Some(&json!(0)));
        assert_eq!(serialized.get("title"), value.get("title"));
        Ok(())
    }

    #[test]
    fn nested_video_uses_half_open_project_time_and_its_own_source_in() -> Result<()> {
        let mut project: Project =
            serde_json::from_str(include_str!("../fixtures/creator/chart.json"))?;
        let inset = |source_in, start, duration| {
            json!({"content":{"type":"video", "source":"insert.mp4", "size":[64,32],
                              "source_in":source_in, "start":start, "duration":duration}})
        };
        project.graphics = serde_json::from_value(json!([
            {"content":{"type":"group","children":[inset(20, 2, 3)]}},
            inset(70, 3, 3)
        ]))?;
        let project = project.resolved(Path::new("assets"))?;
        for (time, expected) in [
            (5, vec![72]),
            (4, vec![22, 71]),
            (3, vec![21, 70]),
            (2, vec![20]),
            (1, vec![]),
            (6, vec![]),
        ] {
            let request = project.request(time)?;
            assert_eq!(request.source_frame, Some(time));
            for input in &request.video_layers {
                assert_eq!(input.source, Path::new("assets/insert.mp4"));
                assert_eq!(input.size, [64, 32]);
            }
            assert_eq!(
                request
                    .video_layers
                    .iter()
                    .map(|v| v.source_frame)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn timed_groups_gate_video_requests_without_slipping_the_media_clock() -> Result<()> {
        let mut project: Project =
            serde_json::from_str(include_str!("../fixtures/creator/chart.json"))?;
        project.graphics = serde_json::from_value(json!([{
            "span":[5,16], "content":{"type":"group","children":[{
                "span":[2,9], "content":{"type":"video","source":"insert.mp4",
                    "size":[64,32],"source_in":20,"start":3,"duration":18}
            }]}
        }]))?;
        let project = project.resolved(Path::new("assets"))?;
        assert_eq!(project.video_clips().len(), 1);
        for (frame, expected) in [
            (4, None),
            (6, None),
            (7, Some(24)),
            (13, Some(30)),
            (14, None),
            (16, None),
        ] {
            let request = project.request(frame)?;
            let ticks: Vec<_> = request
                .video_layers
                .iter()
                .map(|layer| layer.source_frame)
                .collect();
            assert_eq!(ticks, expected.into_iter().collect::<Vec<_>>());
        }
        Ok(())
    }

    #[test]
    fn video_layer_rejects_unreachable_ranges_and_invalid_dimensions() -> Result<()> {
        let mut project: Project =
            serde_json::from_str(include_str!("../fixtures/creator/chart.json"))?;
        let end = project.duration()?;
        for (start, duration, source_in, size) in [
            (0, 0, 0, [16, 8]),
            (end, 1, 0, [16, 8]),
            (u32::MAX, 2, 0, [16, 8]),
            (1, 2, u32::MAX, [16, 8]),
            (1, 2, 0, [0, 8]),
        ] {
            project.graphics = serde_json::from_value(json!([{"content":{
                "type":"video", "source":"clip.mp4", "size":size, "source_in":source_in,
                "start":start, "duration":duration
            }}]))?;
            assert!(project.validate().is_err());
        }
        Ok(())
    }
}
