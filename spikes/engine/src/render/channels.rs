//! Keyframed scalar properties on the owning layer's local clock.
use super::affine::SampledPose;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// Finite values with consistent hashing for document and artwork identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct Scalar(f32);

impl TryFrom<f32> for Scalar {
    type Error = anyhow::Error;
    fn try_from(value: f32) -> Result<Self> {
        ensure!(value.is_finite(), "expected a finite channel value");
        Ok(Self(if value == 0. { 0. } else { value }))
    }
}
impl From<Scalar> for f32 {
    fn from(value: Scalar) -> Self {
        value.0
    }
}
impl Eq for Scalar {}
impl Hash for Scalar {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    #[default]
    Linear,
    Hold,
    Cubic([Scalar; 4]),
}

impl Timing {
    fn is_linear(&self) -> bool {
        matches!(self, Self::Linear)
    }

    fn progress(self, elapsed: f64) -> f64 {
        match self {
            Self::Linear => elapsed,
            Self::Hold => 0.,
            Self::Cubic(points) => {
                let [x1, y1, x2, y2] = points.map(|n| f64::from(n.0));
                let cubic = |t: f64, p1, p2| {
                    let s = 1. - t;
                    3. * s * s * t * p1 + 3. * s * t * t * p2 + t * t * t
                };
                // Elapsed time is the x coordinate, not the curve parameter.
                // Bisection also handles a vertical tangent (zero dx/dt).
                let (mut low, mut high) = (0., 1.);
                for _ in 0..44 {
                    let middle = (low + high) * 0.5;
                    let x = cubic(middle, x1, x2);
                    if x == elapsed {
                        return cubic(middle, y1, y2);
                    } else if x < elapsed {
                        low = middle;
                    } else {
                        high = middle;
                    }
                }
                cubic((low + high) * 0.5, y1, y2)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe<T> {
    pub frame: u32,
    pub value: T,
    #[serde(default, skip_serializing_if = "Timing::is_linear")]
    pub ease: Timing,
}

pub type ScalarKey = Keyframe<Scalar>;

impl Scalar {
    pub(crate) fn interpolate(self, other: Self, t: f64) -> Self {
        Self(lerp(self.0, other.0, t))
    }
}

fn lerp(a: f32, b: f32, t: f64) -> f32 {
    (f64::from(a) * (1. - t) + f64::from(b) * t) as f32
}

pub(crate) fn sample_keys<T: Copy>(
    keys: &[Keyframe<T>],
    frame: u32,
    fraction: f64,
    interpolate: impl Fn(T, T, f64) -> T,
) -> T {
    let next = keys.partition_point(|key| key.frame <= frame);
    if next == 0 {
        return keys[0].value;
    }
    let a = keys[next - 1];
    let Some(b) = keys.get(next) else {
        return a.value;
    };
    if a.frame == frame && fraction == 0. {
        return a.value;
    }
    let elapsed = (f64::from(frame - a.frame) + fraction) / f64::from(b.frame - a.frame);
    let t = a.ease.progress(elapsed);
    interpolate(a.value, b.value, t)
}

pub(crate) fn sample_scalar(keys: &[ScalarKey], frame: u32, fraction: f64) -> f32 {
    sample_keys(keys, frame, fraction, Scalar::interpolate).into()
}

pub(crate) fn validate_keys<T>(name: &str, keys: &[Keyframe<T>]) -> Result<()> {
    keys.first()
        .with_context(|| format!("{name} channel requires at least one key"))?;
    ensure!(
        keys.windows(2).all(|pair| pair[0].frame < pair[1].frame),
        "{name} channel frames must strictly increase"
    );
    for key in keys {
        if let Timing::Cubic(controls) = key.ease {
            ensure!(
                controls.iter().all(|p| (0.0..=1.0).contains(&p.0)),
                "{name} cubic controls must be between 0 and 1"
            );
        }
    }
    Ok(())
}

/// An authored scalar or local keyframes. T enforces the property's value domain.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AnimatedScalar<T> {
    Value(T),
    Keys(Vec<Keyframe<T>>),
}

impl<T: Default> Default for AnimatedScalar<T> {
    fn default() -> Self {
        Self::Value(T::default())
    }
}

impl<T: TryFrom<f32, Error = anyhow::Error>> TryFrom<f32> for AnimatedScalar<T> {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self> {
        Ok(Self::Value(value.try_into()?))
    }
}

impl<T: Copy + Into<f32> + TryFrom<f32, Error = anyhow::Error>> AnimatedScalar<T> {
    pub(crate) fn at(&self, frame: u32, fraction: f64) -> T {
        match self {
            Self::Value(value) => *value,
            Self::Keys(keys) => sample_keys(keys, frame, fraction, |a, b, t| {
                T::try_from(lerp(a.into(), b.into(), t))
                    .expect("bounded interpolation of valid scalar keys")
            }),
        }
    }

    pub(super) fn sampled(&self, frame: u32, fraction: f64) -> Self {
        Self::Value(self.at(frame, fraction))
    }

    pub(crate) fn range(&self, name: &str) -> Result<[f32; 2]> {
        match self {
            Self::Value(value) => Ok([(*value).into(); 2]),
            Self::Keys(keys) => {
                validate_keys(name, keys)?;
                Ok(keys
                    .iter()
                    .fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], key| {
                        let value = key.value.into();
                        [lo.min(value), hi.max(value)]
                    }))
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyChannels {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_x: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_y: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Vec<ScalarKey>>,
}

impl PropertyChannels {
    pub(crate) fn has_spatial(&self) -> bool {
        self.x.is_some()
            || self.y.is_some()
            || self.scale.is_some()
            || self.scale_x.is_some()
            || self.scale_y.is_some()
            || self.rotation.is_some()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.scale.is_none() || (self.scale_x.is_none() && self.scale_y.is_none()),
            "uniform and independent scale channels cannot be combined"
        );
        let mut used = false;
        for (name, track) in [
            ("x", &self.x),
            ("y", &self.y),
            ("scale", &self.scale),
            ("scale_x", &self.scale_x),
            ("scale_y", &self.scale_y),
            ("rotation", &self.rotation),
            ("opacity", &self.opacity),
        ] {
            let Some(keys) = track else { continue };
            used = true;
            validate_keys(name, keys)?;
            for key in keys {
                match name {
                    "scale" | "scale_x" | "scale_y" => {
                        ensure!(key.value.0 > 0., "scale channel must be positive")
                    }
                    "opacity" => ensure!(
                        (0.0..=1.0).contains(&key.value.0),
                        "opacity channel must be between 0 and 1"
                    ),
                    _ => {}
                }
            }
        }
        ensure!(used, "channels requires at least one property");
        Ok(())
    }

    pub(crate) fn sample(&self, frame: u32, fraction: f64) -> SampledPose {
        let value = |track: &Option<Vec<ScalarKey>>, default| {
            track
                .as_ref()
                .map_or(default, |keys| sample_scalar(keys, frame, fraction))
        };
        let uniform = value(&self.scale, 1.);
        SampledPose {
            x: value(&self.x, 0.),
            y: value(&self.y, 0.),
            scale_axes: [value(&self.scale_x, uniform), value(&self.scale_y, uniform)],
            rotation: value(&self.rotation, 0.),
            opacity: value(&self.opacity, 1.),
        }
    }

    /// The extrema of independent bounded scalar channels. These poses bound
    /// fixed-angle affine motion. An animated angle also requires bounding
    /// intermediate orientations when reserving effect scratch.
    pub(crate) fn spatial_extrema(&self) -> impl Iterator<Item = SampledPose> + use<> {
        let range = |track: &Option<Vec<ScalarKey>>, default| {
            let Some(keys) = track else {
                return [default, default];
            };
            let first = keys[0].value.0;
            keys.iter().fold([first, first], |[min, max], key| {
                [min.min(key.value.0), max.max(key.value.0)]
            })
        };
        let ranges = [
            range(&self.x, 0.),
            range(&self.y, 0.),
            range(&self.scale, 1.),
            range(&self.scale_x, 1.),
            range(&self.scale_y, 1.),
            range(&self.rotation, 0.),
        ];
        // Enumerate two corners only for properties that actually vary.
        let mut count = 1;
        for [lo, hi] in ranges {
            if lo != hi {
                count *= 2;
            }
        }
        (0..count).map(move |mut corner| {
            let [x, y, scale, sx, sy, rotation] = ranges.map(|[lo, hi]| {
                if lo == hi {
                    return lo;
                }
                let value = if corner % 2 == 0 { lo } else { hi };
                corner /= 2;
                value
            });
            SampledPose {
                x,
                y,
                scale_axes: [sx * scale, sy * scale],
                rotation,
                ..SampledPose::default()
            }
        })
    }
}
