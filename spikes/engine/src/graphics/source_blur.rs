use super::{AnimatedScalar, Scalar};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Smooth coverage outside a horizontal band, measured in the source's local pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FocusBand {
    pub center_y: AnimatedScalar<Scalar>,
    pub half_width: AnimatedScalar<Scalar>,
    pub feather: AnimatedScalar<Scalar>,
}

/// Gaussian on a drawable's own raster, clamped at its edges. Amount and coverage
/// mix the sharp and blurred source in linear premultiplied RGBA.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBlur {
    /// Gaussian standard deviation in source pixels.
    pub sigma: AnimatedScalar<Scalar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<AnimatedScalar<Scalar>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_band: Option<FocusBand>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SampledSourceBlur {
    pub sigma: f32,
    pub amount: f32,
    /// Center y, sharp half-width, then smooth falloff width in source pixels.
    pub focus_band: Option<[f32; 3]>,
}

impl SourceBlur {
    pub(super) fn validate(&self) -> Result<()> {
        let [lo, hi] = self.sigma.range("source blur sigma")?;
        ensure!(
            lo >= 0. && hi <= 32.,
            "source blur sigma must be between 0 and 32 source pixels"
        );
        if let Some(amount) = &self.amount {
            let [lo, hi] = amount.range("source blur amount")?;
            ensure!(
                lo >= 0. && hi <= 1.,
                "source blur amount must be between 0 and 1"
            );
        }
        if let Some(band) = &self.focus_band {
            band.center_y.range("focus band center-y")?;
            ensure!(
                band.half_width.range("focus band half-width")?[0] >= 0.,
                "focus band half-width must be nonnegative"
            );
            ensure!(
                band.feather.range("focus band feather")?[0] > 0.,
                "focus band feather must be positive"
            );
        }
        Ok(())
    }

    pub(crate) fn sample(&self, frame: u32, fraction: f64) -> SampledSourceBlur {
        let value = |channel: &AnimatedScalar<Scalar>| f32::from(channel.at(frame, fraction));
        SampledSourceBlur {
            sigma: value(&self.sigma),
            amount: self.amount.as_ref().map_or(1., value),
            focus_band: self.focus_band.as_ref().map(|band| {
                [
                    value(&band.center_y),
                    value(&band.half_width),
                    value(&band.feather),
                ]
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn radius_and_coverage_follow_the_same_fractional_source_clock() -> Result<()> {
        let blur: SourceBlur = serde_json::from_value(json!({
            "sigma":[{"frame":2,"value":0},{"frame":4,"value":32}],
            "amount":[{"frame":0,"value":0.8,"ease":"hold"},{"frame":4,"value":0.1}],
            "focus_band":{
                "center_y":[{"frame":0,"value":-4},{"frame":4,"value":4}],
                "half_width":0.5,"feather":8
            }
        }))?;
        blur.validate()?;
        let sample = blur.sample(2, 0.5);
        assert_eq!(sample.sigma, 8.);
        assert_eq!(sample.amount, 0.8);
        assert_eq!(sample.focus_band, Some([1., 0.5, 8.]));
        assert_eq!(blur.sample(0, 0.).sigma, 0.);
        assert_eq!(blur.sample(9, 0.).amount, 0.1);
        let uniform: SourceBlur = serde_json::from_value(json!({"sigma":4}))?;
        assert_eq!(uniform.sample(3, 0.).amount, 1.);
        assert!(uniform.sample(3, 0.).focus_band.is_none());
        Ok(())
    }
}
