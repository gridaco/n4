//! Fixed-topology cubic contours in drawable-local coordinates. Each contour starts
//! with an anchor, followed by a pair of controls and an endpoint for each segment.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use skia_safe::{Path as SkPath, PathBuilder};
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PathData {
    Svg(String),
    Animated(CurveMotion),
    Cubic(CubicContour),
}

impl From<String> for PathData {
    fn from(d: String) -> Self {
        Self::Svg(d)
    }
}

impl From<&str> for PathData {
    fn from(d: &str) -> Self {
        Self::Svg(d.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CubicContour {
    pub points: Vec<CurvePoint>,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurveMotion {
    pub keys: Vec<CurveKey>,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurveKey {
    pub frame: u32,
    pub points: Vec<CurvePoint>,
    /// Interpolation from this pose to the following pose.
    #[serde(default)]
    pub ease: CurveEase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveEase {
    #[default]
    Linear,
    Smooth,
    /// Time-aware Hermite through consecutive spline poses. Endpoint tangents are zero.
    Spline,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "[f32; 2]", into = "[f32; 2]")]
pub struct CurvePoint([f32; 2]);

impl TryFrom<[f32; 2]> for CurvePoint {
    type Error = anyhow::Error;
    fn try_from(value: [f32; 2]) -> Result<Self> {
        ensure!(value.iter().all(|v| v.is_finite()), "nonfinite curve point");
        Ok(Self(value))
    }
}

impl From<CurvePoint> for [f32; 2] {
    fn from(value: CurvePoint) -> Self {
        value.0
    }
}

impl Eq for CurvePoint {}
impl Hash for CurvePoint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for c in self.0 {
            (if c == 0. { 0 } else { c.to_bits() }).hash(state)
        }
    }
}

impl CurvePoint {
    fn mix(self, other: Self, t: f64) -> Self {
        Self(std::array::from_fn(|i| {
            let start = f64::from(self.0[i]);
            (start + (f64::from(other.0[i]) - start) * t) as f32
        }))
    }

    fn offset(self, start: Self, end: Self, scale: f64) -> Self {
        Self(std::array::from_fn(|axis| {
            (f64::from(self.0[axis]) + (f64::from(end.0[axis]) - f64::from(start.0[axis])) * scale)
                as f32
        }))
    }

    fn xy(self) -> (f32, f32) {
        (self.0[0], self.0[1])
    }
}

fn validate_count(points: &[CurvePoint]) -> Result<()> {
    ensure!(
        points.len() >= 4 && (points.len() - 1).is_multiple_of(3),
        "a cubic contour needs one start point and three points per segment"
    );
    Ok(())
}

impl CubicContour {
    pub(super) fn geometry(&self) -> Result<SkPath> {
        validate_count(&self.points)?;
        let mut geometry = PathBuilder::new();
        geometry.move_to(self.points[0].xy());
        for &[c1, c2, end] in self.points[1..].as_chunks::<3>().0 {
            geometry.cubic_to(c1.xy(), c2.xy(), end.xy());
        }
        if self.closed {
            geometry.close();
        }
        Ok(geometry.detach())
    }
}

impl CurveMotion {
    pub(super) fn validate(&self) -> Result<()> {
        let first = self
            .keys
            .first()
            .ok_or_else(|| anyhow::anyhow!("empty curve key list"))?;
        validate_count(&first.points)?;
        ensure!(
            self.keys
                .windows(2)
                .all(|pair| pair[0].frame < pair[1].frame),
            "curve keys require strictly increasing local frames"
        );
        ensure!(
            self.keys
                .iter()
                .all(|key| key.points.len() == first.points.len()),
            "curve poses require matching cubic topology"
        );
        Ok(())
    }

    pub(crate) fn sample(&self, frame: u32, fraction: f64) -> CubicContour {
        let next = self.keys.partition_point(|key| key.frame <= frame);
        let from = &self.keys[next.saturating_sub(1)];
        let points = match self.keys.get(next) {
            Some(to) if next > 0 && from.ease != CurveEase::Hold => {
                let t =
                    (f64::from(frame - from.frame) + fraction) / f64::from(to.frame - from.frame);
                if from.ease == CurveEase::Spline {
                    let u = 1. - t;
                    let weights = [u.powi(3), 3. * u * u * t, 3. * u * t * t, t.powi(3)];
                    self.spline_controls(next - 1)
                        .map(|[a, b, c, d]| {
                            // Bernstein weights are shared by every coordinate. Splitting
                            // a spatial cubic therefore commutes with the time interpolation.
                            CurvePoint(std::array::from_fn(|axis| {
                                (weights[0] * f64::from(a.0[axis])
                                    + weights[1] * f64::from(b.0[axis])
                                    + weights[2] * f64::from(c.0[axis])
                                    + weights[3] * f64::from(d.0[axis]))
                                    as f32
                            }))
                        })
                        .collect()
                } else {
                    let t = if from.ease == CurveEase::Smooth {
                        t * t * (3. - 2. * t)
                    } else {
                        t
                    };
                    from.points
                        .iter()
                        .zip(&to.points)
                        .map(|(&a, &b)| a.mix(b, t))
                        .collect()
                }
            }
            _ => from.points.clone(),
        };
        CubicContour {
            points,
            closed: self.closed,
        }
    }

    /// Temporal Bézier control poses for this interval. The handles use the
    /// central secants measured in local frames. Close an ease/hold boundary
    /// with zero velocity so the preceding or following held pose stays still.
    pub(super) fn spline_controls(&self, i: usize) -> impl Iterator<Item = [CurvePoint; 4]> + '_ {
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        let h = f64::from(b.frame - a.frame) / 3.;
        let previous = i
            .checked_sub(1)
            .map(|j| &self.keys[j])
            .filter(|p| p.ease == CurveEase::Spline);
        let next = self.keys.get(i + 2).filter(|_| b.ease == CurveEase::Spline);
        a.points
            .iter()
            .zip(&b.points)
            .enumerate()
            .map(move |(j, (&start, &end))| {
                let leaving = previous.map_or(start, |prev| {
                    let factor = h / f64::from(b.frame - prev.frame);
                    start.offset(prev.points[j], end, factor)
                });
                let arriving = next.map_or(end, |next| {
                    let factor = -h / f64::from(next.frame - a.frame);
                    end.offset(start, next.points[j], factor)
                });
                [start, leaving, arriving, end]
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_pose(frame: u32, offset: f32, ease: CurveEase) -> CurveKey {
        CurveKey {
            frame,
            ease,
            points: vec![
                CurvePoint([0., 0.]),
                CurvePoint([10., offset]),
                CurvePoint([20., 5. + offset]),
                CurvePoint([30., 5.]),
            ],
        }
    }

    #[test]
    fn discrete_and_subframe_samples_preserve_the_anchor_and_move_its_handle() -> Result<()> {
        let mut motion = CurveMotion {
            closed: false,
            keys: vec![
                line_pose(5, 0., CurveEase::Hold),
                line_pose(10, 20., CurveEase::Linear),
                line_pose(14, 60., CurveEase::Smooth),
                line_pose(18, 100., CurveEase::Linear),
            ],
        };
        motion.validate()?;
        for (f, fraction, y) in [
            (20, 0., 100.),
            (9, 0.9, 0.),
            (11, 0.5, 35.),
            (10, 0., 20.),
            (15, 0., 66.25),
            (0, 0., 0.),
            (11, 0.5, 35.),
        ] {
            let p = motion.sample(f, fraction);
            assert_eq!(p.points[0].0, [0., 0.]);
            assert_eq!(p.points[1].0, [10., y]);
            assert_eq!(p.points[3].0, [30., 5.]);
        }
        motion.keys[1].points.pop();
        assert!(motion.validate().is_err());
        motion.keys[1] = line_pose(5, 20., CurveEase::Linear);
        assert!(motion.validate().is_err());
        Ok(())
    }

    #[test]
    fn spline_velocity_respects_unequal_key_times_and_commutes_with_a_curve_split() {
        let mut motion = CurveMotion {
            closed: false,
            keys: vec![
                line_pose(0, 0., CurveEase::Spline),
                line_pose(5, 10., CurveEase::Spline),
                line_pose(15, 40., CurveEase::Spline),
                line_pose(18, 40., CurveEase::Hold),
            ],
        };
        // The analytic time derivative at frame 5 is (40 - 0)/(15 - 0).
        let handle = |frame, fraction| motion.sample(frame, fraction).points[1].0[1];
        let before = (handle(5, 0.) - handle(4, 0.999)) * 1000.;
        let after = (handle(5, 0.001) - handle(5, 0.)) * 1000.;
        assert!((before - 40. / 15.).abs() < 0.01);
        assert!((after - before).abs() < 0.01);
        let left_half = |p: &[CurvePoint]| {
            let ab = p[0].mix(p[1], 0.5);
            let bc = p[1].mix(p[2], 0.5);
            let cd = p[2].mix(p[3], 0.5);
            let abc = ab.mix(bc, 0.5);
            vec![p[0], ab, abc, abc.mix(bc.mix(cd, 0.5), 0.5)]
        };
        let expected = left_half(&motion.sample(9, 0.375).points);
        for key in &mut motion.keys {
            key.points = left_half(&key.points);
        }
        let actual = motion.sample(9, 0.375);
        for (a, b) in actual.points.iter().zip(&expected) {
            for axis in 0..2 {
                assert!((a.0[axis] - b.0[axis]).abs() < 0.00001);
            }
        }
    }
}
