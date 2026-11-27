//! Source-local image operations in authored order.
use super::{AnimatedScalar, Scalar, SourceBlur};
use crate::render::channels::{sample_keys, validate_keys};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceEffect {
    SourceBlur(SourceBlur),
    RgbGrade(RgbGrade),
}

/// Channel gain and normalized offset on straight, encoded sRGB. Each
/// operation clamps its RGB result to [0, 1] and retains the source alpha.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RgbGrade {
    #[serde(default = "unit_gain")]
    pub gain: AnimatedScalar<[Scalar; 3]>,
    #[serde(default)]
    pub offset: AnimatedScalar<[Scalar; 3]>,
}

fn unit_gain() -> AnimatedScalar<[Scalar; 3]> {
    AnimatedScalar::Value([1f32.try_into().expect("finite unit gain"); 3])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SampledGrade {
    pub gain: [f32; 3],
    pub offset: [f32; 3],
}

impl SampledGrade {
    pub(crate) const IDENTITY: Self = Self {
        gain: [1.; 3],
        offset: [0.; 3],
    };
}

impl RgbGrade {
    pub(super) fn validate(&self) -> Result<()> {
        for (name, values) in [("grade gain", &self.gain), ("grade offset", &self.offset)] {
            if let AnimatedScalar::Keys(keys) = values {
                validate_keys(name, keys)?;
            }
        }
        Ok(())
    }

    pub(crate) fn always_identity(&self) -> bool {
        [(&self.gain, 1.), (&self.offset, 0.)]
            .into_iter()
            .all(|(values, neutral)| {
                let matches =
                    |v: &[Scalar; 3]| v.iter().all(|&channel| f32::from(channel) == neutral);
                match values {
                    AnimatedScalar::Value(v) => matches(v),
                    AnimatedScalar::Keys(keys) => keys.iter().all(|k| matches(&k.value)),
                }
            })
    }

    pub(crate) fn sample(&self, frame: u32, fraction: f64) -> SampledGrade {
        let sample = |values: &AnimatedScalar<[Scalar; 3]>| {
            let value = match values {
                AnimatedScalar::Value(value) => *value,
                AnimatedScalar::Keys(keys) => sample_keys(keys, frame, fraction, |a, b, t| {
                    std::array::from_fn(|i| a[i].interpolate(b[i], t))
                }),
            };
            value.map(f32::from)
        };
        SampledGrade {
            gain: sample(&self.gain),
            offset: sample(&self.offset),
        }
    }
}
