//! A detail declares viewpoints; a master placement selects one for each output frame.
use crate::Shot;
use crate::graphics::{AnimatedScalar, Content, Graphic, SampleTime, Scalar, Transform};
use crate::render::affine::{SampledPose, SampledTransform};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

type Channel = AnimatedScalar<Scalar>;
pub(crate) type OrthoRanges = [[f32; 2]; 4];

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "projection", rename_all = "snake_case", deny_unknown_fields)]
pub enum Camera {
    Orthographic {
        id: String,
        #[serde(default)]
        center_x: Option<Channel>,
        #[serde(default)]
        center_y: Option<Channel>,
        #[serde(default)]
        zoom: Option<Channel>,
        #[serde(default)]
        roll: Option<Channel>,
    },
    Perspective {
        id: String,
        position: [Channel; 3],
        #[serde(default)]
        rotation: [Channel; 3],
        fov_y: Channel,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraCut {
    pub start: u32,
    pub end: u32,
    pub camera: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PerspectiveView {
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub fov_y_degrees: f32,
}

impl Camera {
    pub fn id(&self) -> &str {
        match self {
            Self::Orthographic { id, .. } | Self::Perspective { id, .. } => id,
        }
    }

    fn validate(&self) -> Result<()> {
        let id = self.id();
        ensure!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "invalid camera ID {id:?}"
        );
        match self {
            Self::Orthographic {
                center_x,
                center_y,
                zoom,
                roll,
                ..
            } => {
                for (name, track) in [
                    ("center-x", center_x),
                    ("center-y", center_y),
                    ("roll", roll),
                ] {
                    if let Some(track) = track {
                        track.range(name)?;
                    }
                }
                if let Some(zoom) = zoom {
                    ensure!(zoom.range("zoom")?[0] > 0., "camera zoom must be positive");
                }
            }
            Self::Perspective {
                position,
                rotation,
                fov_y,
                ..
            } => {
                for axis in position.iter().chain(rotation) {
                    axis.range("camera pose")?;
                }
                let [lo, hi] = fov_y.range("fov-y")?;
                ensure!(
                    lo > 0. && hi < 180.,
                    "camera fov-y must lie between 0 and 180 degrees"
                );
            }
        }
        Ok(())
    }

    /// Independent reachable ranges, used to admit camera-framed GPU scratch before an edit.
    pub(crate) fn ortho_ranges(&self, size: [u32; 2]) -> Result<Option<OrthoRanges>> {
        let Self::Orthographic {
            center_x,
            center_y,
            zoom,
            roll,
            ..
        } = self
        else {
            return Ok(None);
        };
        let range = |track: &Option<Channel>, default| {
            track
                .as_ref()
                .map_or(Ok([default; 2]), |channel| channel.range("camera property"))
        };
        let [cx, cy] = size.map(|v| v as f32 * 0.5);
        Ok(Some([
            range(center_x, cx)?,
            range(center_y, cy)?,
            range(zoom, 1.)?,
            range(roll, 0.)?,
        ]))
    }

    pub(crate) fn affine(&self, time: SampleTime, size: [u32; 2]) -> Option<SampledTransform> {
        let Self::Orthographic {
            center_x,
            center_y,
            zoom,
            roll,
            ..
        } = self
        else {
            return None;
        };
        let value = |track: &Option<Channel>, default: f32| {
            track
                .as_ref()
                .map_or(default, |c| f32::from(c.at(time.frame, time.fraction())))
        };
        let half = size.map(|d| d as f32 * 0.5);
        let center = [value(center_x, half[0]), value(center_y, half[1])];
        let zoom = value(zoom, 1.);
        let pose = SampledPose {
            x: half[0] - center[0],
            y: half[1] - center[1],
            rotation: -value(roll, 0.),
            scale_axes: [zoom; 2],
            opacity: 1.,
        };
        SampledTransform::new(
            Some(Transform {
                pivot: center,
                ..Default::default()
            }),
            pose,
        )
    }

    pub(crate) fn perspective(&self, time: SampleTime) -> Option<PerspectiveView> {
        let Self::Perspective {
            position,
            rotation,
            fov_y,
            ..
        } = self
        else {
            return None;
        };
        Some(PerspectiveView {
            position: std::array::from_fn(|i| position[i].at(time.frame, time.fraction()).into()),
            rotation_degrees: std::array::from_fn(|i| {
                rotation[i].at(time.frame, time.fraction()).into()
            }),
            fov_y_degrees: fov_y.at(time.frame, time.fraction()).into(),
        })
    }
}

pub(crate) fn contains_scene3d(layers: &[Graphic]) -> bool {
    layers.iter().any(|layer| match &layer.content {
        Content::Scene3d(_) => true,
        Content::Group { children } | Content::TransformGroup { children } => {
            contains_scene3d(children)
        }
        _ => false,
    })
}

impl Shot {
    pub(crate) fn validate_cameras(&self) -> Result<()> {
        let mut ids = std::collections::HashSet::new();
        for camera in &self.cameras {
            camera
                .validate()
                .with_context(|| format!("camera {:?}", camera.id()))?;
            ensure!(
                ids.insert(camera.id()),
                "duplicate camera ID {:?}",
                camera.id()
            );
        }
        if let Some(id) = &self.camera {
            ensure!(ids.contains(id.as_str()), "unknown default camera {id:?}");
        }
        let mut next = 0;
        for cut in &self.camera_cuts {
            ensure!(
                cut.start == next && cut.end > next && cut.end <= self.duration,
                "camera cuts must cover the placement exactly once in order"
            );
            ensure!(
                ids.contains(cut.camera.as_str()),
                "unknown camera {:?}",
                cut.camera
            );
            next = cut.end;
        }
        ensure!(
            self.camera_cuts.is_empty() || next == self.duration,
            "camera cuts do not cover the whole placement"
        );
        if contains_scene3d(&self.graphics) {
            let perspective = |id: &str| {
                self.cameras
                    .iter()
                    .any(|c| matches!(c, Camera::Perspective { .. }) && c.id() == id)
            };
            let valid = if self.camera_cuts.is_empty() {
                self.camera.as_deref().is_some_and(perspective)
            } else {
                self.camera_cuts.iter().all(|cut| perspective(&cut.camera))
            };
            ensure!(
                valid,
                "scene3d requires a selected perspective camera for the entire picture placement"
            );
        }
        Ok(())
    }

    fn selected(&self, id: &str) -> bool {
        if self.camera_cuts.is_empty() {
            self.camera.as_deref() == Some(id)
        } else {
            self.camera_cuts.iter().any(|cut| cut.camera == id)
        }
    }

    pub fn camera_at(&self, frame: u32) -> Option<&Camera> {
        let id = if self.camera_cuts.is_empty() {
            self.camera.as_deref()?
        } else {
            let index = self.camera_cuts.partition_point(|cut| cut.end <= frame);
            self.camera_cuts.get(index)?.camera.as_str()
        };
        self.cameras.iter().find(|camera| camera.id() == id)
    }
}

impl crate::Project {
    /// Visit each framing a drawable may have. High-resolution filter admission uses
    /// this conservative range even when a particular cut only selects part of a move.
    pub(crate) fn visit_framed_scopes(
        &self,
        mut visit: impl FnMut(&[Graphic], u32, [u32; 2], Option<OrthoRanges>) -> Result<()>,
    ) -> Result<()> {
        fn live_details(
            layers: &[Graphic],
            duration: u32,
            visit: &mut impl FnMut(&[Graphic], u32, [u32; 2], Option<OrthoRanges>) -> Result<()>,
        ) -> Result<()> {
            for layer in layers {
                let local = layer.span.map_or(duration, |[start, end]| end - start);
                match &layer.content {
                    Content::Group { children } | Content::TransformGroup { children } => {
                        live_details(children, local, visit)?
                    }
                    Content::Scene3d(scene) => {
                        for surface in scene.models.iter().flat_map(|model| &model.surfaces) {
                            visit(&surface.layers, local, surface.size, None)?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        for shot in &self.shots {
            live_details(&shot.graphics, shot.duration, &mut visit)?;
            if shot.camera.is_none() && shot.camera_cuts.is_empty() {
                visit(&shot.graphics, shot.duration, self.size, None)?;
            } else {
                for camera in shot
                    .cameras
                    .iter()
                    .filter(|camera| shot.selected(camera.id()))
                {
                    visit(
                        &shot.graphics,
                        shot.duration,
                        self.size,
                        camera.ortho_ranges(self.size)?,
                    )?;
                }
            }
        }
        for placement in &self.overlays {
            visit(&placement.graphics, placement.duration, self.size, None)?;
        }
        visit(&self.graphics, self.duration()?, self.size, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::Exposure;
    use serde_json::json;

    #[test]
    fn camera_roll_rotates_the_world_about_the_authored_center_and_pan_is_inverse() -> Result<()> {
        for (center_x, roll, expected) in [
            (1000, 0, [980., 560.]),
            (1010, 0, [960., 560.]),
            (1000, 90, [980., 520.]),
        ] {
            let view: Camera = serde_json::from_value(json!({
                "id":"view","projection":"orthographic","center_x":center_x,"center_y":540,
                "zoom":2,"roll":roll
            }))?;
            let transform = view
                .affine(SampleTime::at(0, None, 0), [1920, 1080])
                .unwrap();
            assert_eq!(transform.map_point([1010., 550.]), expected);
        }
        Ok(())
    }

    #[test]
    fn switching_angles_preserves_detail_time_and_samples_the_pose_within_the_shutter() -> Result<()>
    {
        let shot: Shot = serde_json::from_value(json!({
            "duration":10,"background":{"color":[0,0,0]},
            "cameras":[{"id":"wide","projection":"orthographic"},
                {"id":"close","projection":"perspective","position":[0,0,22],
                 "fov_y":[{"frame":0,"value":20},{"frame":10,"value":30}]}],
            "camera_cuts":[{"camera":"wide","start":0,"end":6},{"camera":"close","start":6,"end":10}]
        }))?;
        shot.validate_cameras()?;
        let selected = shot.camera_at(6).unwrap();
        let shutter = Some(Exposure {
            samples: 2,
            angle_degrees: 360,
        });
        for (sample, expected) in [(0, 26.25), (1, 26.75)] {
            let view = selected
                .perspective(SampleTime::at(6, shutter, sample))
                .unwrap();
            assert_eq!(view.fov_y_degrees, expected);
        }
        Ok(())
    }
}
