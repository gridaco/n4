//! Authored local poses and their evaluated two-dimensional affine maps.
use super::channels::PropertyChannels;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// Scale on local axes, then rotate clockwise about the local pivot. Distances
/// are pixels, angles are degrees, and a scale of one preserves the dimension.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Transform {
    pub pivot: [f32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_degrees: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_y: Option<f32>,
}

impl Transform {
    fn bits(self) -> ([u32; 2], [Option<u32>; 4]) {
        (
            self.pivot.map(f32::to_bits),
            [
                self.rotation_degrees,
                self.scale,
                self.scale_x,
                self.scale_y,
            ]
            .map(|n| n.map(f32::to_bits)),
        )
    }

    pub(crate) fn validate(
        self,
        channels: Option<&PropertyChannels>,
        poses: &[MotionKey],
    ) -> Result<()> {
        ensure!(
            self.pivot.into_iter().all(f32::is_finite)
                && self.rotation_degrees.is_none_or(f32::is_finite),
            "transform pivot and rotation must be finite"
        );
        ensure!(
            [self.scale, self.scale_x, self.scale_y]
                .into_iter()
                .flatten()
                .all(|v| v.is_finite() && v > 0.),
            "transform scale must be finite and positive"
        );
        let uniform = self.scale.is_some();
        let axes = self.scale_x.is_some() || self.scale_y.is_some();
        ensure!(
            !(uniform && axes),
            "uniform and independent scale cannot be combined"
        );
        if let Some(c) = channels {
            ensure!(
                !(self.rotation_degrees.is_some() && c.rotation.is_some()
                    || uniform
                        && (c.scale.is_some() || c.scale_x.is_some() || c.scale_y.is_some())
                    || axes && c.scale.is_some()
                    || self.scale_x.is_some() && c.scale_x.is_some()
                    || self.scale_y.is_some() && c.scale_y.is_some()),
                "static and animated transform properties conflict"
            );
        }
        ensure!(
            !(uniform || axes) || poses.iter().all(|p| p.scale == 1.),
            "static and animated scale conflict"
        );
        Ok(())
    }

    pub(crate) fn scales(self) -> [f64; 2] {
        [self.scale_x, self.scale_y].map(|axis| f64::from(axis.or(self.scale).unwrap_or(1.)))
    }

    pub(crate) fn is_identity(self) -> bool {
        self.rotation_degrees.unwrap_or_default() % 360. == 0. && self.scales() == [1., 1.]
    }
}

impl PartialEq for Transform {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}
impl Eq for Transform {}
impl Hash for Transform {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits().hash(state);
    }
}

// Preserve exact multiples of a quarter turn on the original pixel grid. Reduce
// only after evaluating the unwrapped angle, including the fractional frame.
fn rotation_axes(degrees: f64) -> (f64, f64) {
    match degrees.rem_euclid(360.) {
        0. => (1., 0.),
        90. => (0., 1.),
        180. => (-1., 0.),
        270. => (0., -1.),
        a => {
            let (s, c) = a.to_radians().sin_cos();
            (c, s)
        }
    }
}

/// An authored grouped pose on the layer's local clock.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MotionKey {
    pub frame: u32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub opacity: f32,
}

impl Default for MotionKey {
    fn default() -> Self {
        Self {
            frame: 0,
            x: 0.,
            y: 0.,
            scale: 1.,
            opacity: 1.,
        }
    }
}

impl MotionKey {
    fn bits(&self) -> [u32; 5] {
        [
            self.frame,
            self.x.to_bits(),
            self.y.to_bits(),
            self.scale.to_bits(),
            self.opacity.to_bits(),
        ]
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.x.is_finite()
                && self.y.is_finite()
                && self.scale.is_finite()
                && self.scale > 0.
                && (0.0..=1.0).contains(&self.opacity),
            "invalid drawable motion key"
        );
        Ok(())
    }

    pub(crate) fn sample(keys: &[Self], frame: u32, fraction: f64) -> Self {
        let next = keys.partition_point(|key| key.frame <= frame);
        if next == 0 {
            return keys.first().copied().unwrap_or_default();
        }
        let a = keys[next - 1];
        let Some(b) = keys.get(next) else { return a };
        let u = (f64::from(frame - a.frame) + fraction) / f64::from(b.frame - a.frame);
        let lerp = |a: f32, b: f32| (f64::from(a) * (1. - u) + f64::from(b) * u) as f32;
        Self {
            frame,
            x: lerp(a.x, b.x),
            y: lerp(a.y, b.y),
            scale: lerp(a.scale, b.scale),
            opacity: lerp(a.opacity, b.opacity),
        }
    }
}

impl PartialEq for MotionKey {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}
impl Eq for MotionKey {}
impl Hash for MotionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits().hash(state);
    }
}

/// The common sample of grouped poses and independent property curves.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SampledPose {
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub scale_axes: [f32; 2],
    pub opacity: f32,
}

impl From<MotionKey> for SampledPose {
    fn from(key: MotionKey) -> Self {
        Self {
            x: key.x,
            y: key.y,
            rotation: 0.,
            scale_axes: [key.scale; 2],
            opacity: key.opacity,
        }
    }
}
impl Default for SampledPose {
    fn default() -> Self {
        MotionKey::default().into()
    }
}
impl SampledPose {
    pub(crate) fn scales(self) -> [f64; 2] {
        self.scale_axes.map(f64::from)
    }
}

/// Rows of the linear map. Translation is expressed relative to the sum of
/// integer layer placements used by the painter. Keep the analytic inverse of
/// each local pose and compose it in reverse order to avoid subtracting nearly
/// equal products to recover a determinant deep in a hierarchy.
type Basis = [[f64; 2]; 2];

fn apply(basis: Basis, [x, y]: [f64; 2]) -> [f64; 2] {
    basis.map(|[a, b]| a * x + b * y)
}

fn product(left: Basis, right: Basis) -> Basis {
    left.map(|[x, y]| {
        [
            x * right[0][0] + y * right[1][0],
            x * right[0][1] + y * right[1][1],
        ]
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SampledTransform {
    basis: Basis,
    inverse: Basis,
    offset: [f64; 2],
}

impl SampledTransform {
    pub(super) const IDENTITY: Self = Self {
        basis: [[1., 0.], [0., 1.]],
        inverse: [[1., 0.], [0., 1.]],
        offset: [0.; 2],
    };

    pub(crate) fn map_point(self, point: [f64; 2]) -> [f64; 2] {
        let [x, y] = apply(self.basis, point);
        [x + self.offset[0], y + self.offset[1]]
    }
    pub(crate) fn inverse_rows(self) -> Basis {
        self.inverse
    }

    pub fn new(base: Option<Transform>, motion: impl Into<SampledPose>) -> Option<Self> {
        let motion = motion.into();
        let base = base.unwrap_or_default();
        let angle =
            f64::from(base.rotation_degrees.unwrap_or_default()) + f64::from(motion.rotation);
        let (c, s) = rotation_axes(angle);
        let [bx, by] = base.scales();
        let [mx, my] = motion.scales();
        let (sx, sy) = (bx * mx, by * my);
        if c == 1. && s == 0. && sx == 1. && sy == 1. && motion.x == 0. && motion.y == 0. {
            return None;
        }
        let basis = [[c * sx, -s * sy], [s * sx, c * sy]];
        let inverse = [[c / sx, s / sx], [-s / sy, c / sy]];
        let [px, py] = base.pivot.map(f64::from);
        let offset = [
            (1. - basis[0][0]) * px - basis[0][1] * py + f64::from(motion.x),
            (1. - basis[1][1]) * py - basis[1][0] * px + f64::from(motion.y),
        ];
        Some(Self {
            basis,
            inverse,
            offset,
        })
    }

    pub fn inherit(
        parent: Option<Self>,
        child_position: [i32; 2],
        child: Option<Self>,
    ) -> Option<Self> {
        let Some(parent) = parent else { return child };
        let child = child.unwrap_or(Self::IDENTITY);
        let [x, y] = child_position.map(f64::from);
        let [tx, ty] = parent.map_point([x + child.offset[0], y + child.offset[1]]);
        Some(Self {
            basis: product(parent.basis, child.basis),
            inverse: product(child.inverse, parent.inverse),
            offset: [tx - x, ty - y],
        })
    }
}

/// The painter's integer origin and inherited affine map for a layer sample.
#[derive(Clone, Copy, Default)]
pub(crate) struct LayerSpace {
    pub origin: [i64; 2],
    pub transform: Option<SampledTransform>,
}

impl LayerSpace {
    pub(crate) fn child(self, position: [i32; 2], transform: Option<SampledTransform>) -> Self {
        Self {
            origin: [
                self.origin[0] + i64::from(position[0]),
                self.origin[1] + i64::from(position[1]),
            ],
            transform: SampledTransform::inherit(self.transform, position, transform),
        }
    }
}

#[cfg(feature = "host-audio")]
impl LayerSpace {
    pub(crate) fn to_composition(self, local: [f64; 2]) -> [f64; 2] {
        let [x, y] = self
            .transform
            .unwrap_or(SampledTransform::IDENTITY)
            .map_point(local);
        [x + self.origin[0] as f64, y + self.origin[1] as f64]
    }

    pub(crate) fn to_local(self, point: [f64; 2]) -> [f64; 2] {
        let transform = self.transform.unwrap_or(SampledTransform::IDENTITY);
        let [x, y] = self.to_composition([0.; 2]);
        apply(transform.inverse, [point[0] - x, point[1] - y])
    }

    pub(crate) fn rows(self) -> [[f64; 3]; 2] {
        let transform = self.transform.unwrap_or(SampledTransform::IDENTITY);
        let [x, y] = self.to_composition([0.; 2]);
        let [[a, b], [c, d]] = transform.basis;
        [[a, b, x], [c, d, y]]
    }
}

/// Conservative lower/upper singular-value bounds and translation radius.
/// All independently animated ancestors can attain different extrema at once.
#[derive(Clone, Copy)]
pub(crate) struct TransformLimits {
    scale_min: f64,
    scale_max: f64,
    condition: f64,
    offset_radius: f64,
}

impl Default for TransformLimits {
    fn default() -> Self {
        Self {
            scale_min: 1.,
            scale_max: 1.,
            condition: 1.,
            offset_radius: 0.,
        }
    }
}

impl TransformLimits {
    pub fn child(self, layer: &crate::graphics::Graphic, origin: [i64; 2]) -> Result<Self> {
        let base = layer.transform.unwrap_or_default();
        let [bx, by] = base.scales();
        let mut extrema = layer.spatial_extrema();
        let first = extrema.next().unwrap_or_default();
        let mut min = f64::INFINITY;
        let mut max = 0_f64;
        let mut condition = 1_f64;
        let mut translation = 0_f64;
        for pose in std::iter::once(first).chain(extrema) {
            let [mx, my] = pose.scales();
            let (sx, sy) = (bx * mx, by * my);
            min = min.min(sx.min(sy));
            max = max.max(sx.max(sy));
            condition = condition.max(sx.max(sy) / sx.min(sy));
            translation = translation.max(f64::from(pose.x).hypot(f64::from(pose.y)));
        }
        let local_radius =
            translation + (1. + max) * f64::from(base.pivot[0]).hypot(f64::from(base.pivot[1]));
        let [x, y] = layer.placed_position().map(f64::from);
        let offset_radius =
            self.offset_radius + (self.scale_max + 1.) * x.hypot(y) + self.scale_max * local_radius;
        let scale_min = self.scale_min * min;
        let scale_max = self.scale_max * max;
        // Transport is f32. Under rotations, a basis whose smallest singular
        // value is below its representation error can lose an entire axis.
        // Uniform zoom has condition one at every time, even over a wide range.
        let condition = self.condition * condition;
        let screen = f64::from(crate::MAX_SURFACE_DIMENSION) * 2.;
        let max_float = f64::from(f32::MAX);
        let origin_distance = (origin[0] as f64).hypot(origin[1] as f64);
        let mask_offset = layer.mask.as_ref().map_or(0., |m| {
            f64::from(m.position[0]).hypot(f64::from(m.position[1]))
        });
        ensure!(
            origin.into_iter().all(|v| i32::try_from(v).is_ok())
                && scale_min > 0.
                && scale_max <= max_float
                && condition * f64::from(f32::EPSILON) < 1.
                && (1. / scale_min) <= max_float
                && origin_distance + offset_radius + scale_max * (screen + mask_offset)
                    <= max_float
                && (origin_distance + offset_radius + screen) / scale_min <= max_float,
            "transform hierarchy exceeds GPU coordinate range"
        );
        Ok(Self {
            scale_min,
            scale_max,
            condition,
            offset_radius,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::Graphic;
    use serde_json::json;

    fn close(actual: [f64; 2], expected: [f64; 2]) {
        for i in 0..2 {
            assert!(
                (actual[i] - expected[i]).abs() < 1e-8,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn nested_transform_matches_separate_spaces_and_has_an_inverse() {
        let base = Transform {
            pivot: [5.25, -2.5],
            scale_x: Some(2.),
            scale_y: Some(1.),
            ..Default::default()
        };
        let parent = SampledTransform::new(
            Some(base),
            MotionKey {
                x: 3.5,
                ..Default::default()
            },
        )
        .unwrap();
        close(parent.map_point([5.25, -2.5]), [8.75, -2.5]);
        let child = SampledTransform::new(
            Some(Transform {
                pivot: [2., 3.],
                rotation_degrees: Some(45.),
                ..Default::default()
            }),
            MotionKey::default(),
        )
        .unwrap();
        let world = SampledTransform::inherit(Some(parent), [13, 9], Some(child)).unwrap();
        // Independent geometric construction in the two local spaces. This
        // corner lies along the rotated local X axis from the child's pivot.
        let r = 0.5_f64.sqrt();
        let expected_abs = [5.25 + 3.5 + 2. * (13. + 2. + r - 5.25), 9. + 3. + r];
        let projected = world.map_point([3., 3.]);
        close([projected[0] + 13., projected[1] + 9.], expected_abs);
        let zero = world.map_point([0., 0.]);
        close(
            apply(
                world.inverse_rows(),
                [projected[0] - zero[0], projected[1] - zero[1]],
            ),
            [3., 3.],
        );
        let u = apply(world.basis, [1., 0.]);
        let v = apply(world.basis, [0., 1.]);
        assert!((u[0] * v[0] + u[1] * v[1] + 1.5).abs() < 1e-10);
    }

    #[test]
    fn turn_count_and_pivot_survive_local_shutter_sampling() -> Result<()> {
        let layer: Graphic = serde_json::from_value(json!({
            "transform":{"pivot":[2.5,3.25], "scale_x":0.5},
            "channels":{"rotation":[{"frame":0,"value":0},{"frame":8,"value":720}],
                        "scale_y":[{"frame":0,"value":1},{"frame":8,"value":3}]},
            "content":{"type":"footage"}
        }))?;
        let base = layer.transform.unwrap();
        base.validate(layer.channels.as_ref(), &layer.motion)?;
        for t in [8., 1., 0., 4., 0.5] {
            let pose = layer.motion_at(t as u32, t % 1.);
            let xf = SampledTransform::new(Some(base), pose).unwrap();
            close(xf.map_point([2.5, 3.25]), [2.5, 3.25]);
            let (s, c) = (t * 90_f64).to_radians().sin_cos();
            close(xf.map_point([3.5, 3.25]), [2.5 + 0.5 * c, 3.25 + 0.5 * s]);
            close(
                xf.map_point([2.5, 4.25]),
                [2.5 - (1. + t / 4.) * s, 3.25 + (1. + t / 4.) * c],
            );
        }
        Ok(())
    }

    #[test]
    fn scale_admission_distinguishes_uniform_zoom_from_collapsing_an_axis() -> Result<()> {
        let layer = |channels| {
            serde_json::from_value::<Graphic>(json!({
                "content":{"type":"footage"}, "channels":channels
            }))
        };
        let keys = json!([{"frame":0,"value":1e-7},{"frame":5,"value":1e7}]);
        let zoom = layer(json!({"scale":keys}))?;
        TransformLimits::default().child(&zoom, [0; 2])?;
        let thin = layer(json!({"scale_x":keys}))?;
        assert!(TransformLimits::default().child(&thin, [0; 2]).is_err());
        Ok(())
    }
}
