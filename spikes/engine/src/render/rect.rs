//! Editable SVG-style rectangle geometry. Sampling resolves local lengths before
//! painting, so a moving fractional edge retains coverage in the raster itself.
use super::{
    channels::{AnimatedScalar, Keyframe, Scalar, sample_keys, validate_keys},
    path::{FillRule, Stroke, default_fill, paint_prepared, prepare_geometry},
};
use crate::{MAX_SURFACE_DIMENSION, graphics::Raster};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use skia_safe::{Path as SkPath, RRect, Rect, Vector};

/// Length in the rectangle's local coordinate system.
pub type RectLength = AnimatedScalar<Scalar>;

/// Clockwise from the upper-left, in drawable-local pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CornerValues {
    Uniform(Scalar),
    Four([Scalar; 4]),
}

impl CornerValues {
    fn corners(self) -> [Scalar; 4] {
        match self {
            Self::Uniform(value) => [value; 4],
            Self::Four(values) => values,
        }
    }

    fn interpolate(self, other: Self, t: f64) -> Self {
        let a = self.corners();
        let b = other.corners();
        Self::Four(std::array::from_fn(|i| a[i].interpolate(b[i], t)))
    }

    fn validate(self) -> Result<()> {
        ensure!(
            self.corners().into_iter().all(|r| f32::from(r) >= 0.),
            "rectangle corner-radius must be nonnegative"
        );
        Ok(())
    }

    fn path(self, rect: Rect) -> SkPath {
        let radii = self.corners().map(|r| f64::from(f32::from(r)));
        let mut factor = 1.0_f64;
        let [tl, tr, br, bl] = radii;
        for (edge, sum) in [
            (rect.width(), tl + tr),
            (rect.height(), tr + br),
            (rect.width(), br + bl),
            (rect.height(), bl + tl),
        ] {
            if sum > 0. {
                factor = factor.min(f64::from(edge) / sum);
            }
        }
        let vectors = radii.map(|r| {
            let used = (r * factor) as f32;
            Vector::new(used, used)
        });
        SkPath::rrect(RRect::new_rect_radii(rect, &vectors), None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CornerRadius {
    Value(CornerValues),
    Keys(Vec<Keyframe<CornerValues>>),
}

impl CornerRadius {
    fn at(&self, frame: u32, fraction: f64) -> CornerValues {
        match self {
            Self::Value(value) => *value,
            Self::Keys(keys) => sample_keys(keys, frame, fraction, CornerValues::interpolate),
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Value(value) => value.validate(),
            Self::Keys(keys) => {
                validate_keys("corner-radius", keys)?;
                keys.iter().try_for_each(|key| key.value.validate())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RectGeometry {
    #[serde(default)]
    pub x: RectLength,
    #[serde(default)]
    pub y: RectLength,
    pub width: RectLength,
    pub height: RectLength,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx: Option<RectLength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ry: Option<RectLength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<CornerRadius>,
}

impl RectGeometry {
    pub(crate) fn animated(&self) -> bool {
        [&self.x, &self.y, &self.width, &self.height]
            .into_iter()
            .chain(self.rx.iter())
            .chain(self.ry.iter())
            .any(|v| matches!(v, RectLength::Keys(_)))
            || matches!(self.corner_radius, Some(CornerRadius::Keys(_)))
    }

    pub(crate) fn sampled(&self, frame: u32, fraction: f64) -> Self {
        let sample = |v: &RectLength| v.sampled(frame, fraction);
        Self {
            x: sample(&self.x),
            y: sample(&self.y),
            width: sample(&self.width),
            height: sample(&self.height),
            rx: self.rx.as_ref().map(sample),
            ry: self.ry.as_ref().map(sample),
            corner_radius: self
                .corner_radius
                .as_ref()
                .map(|r| CornerRadius::Value(r.at(frame, fraction))),
        }
    }

    fn path(&self) -> SkPath {
        let constant = |v: &RectLength| f32::from(v.at(0, 0.));
        let (w, h) = (constant(&self.width), constant(&self.height));
        if w == 0. || h == 0. {
            return SkPath::new();
        }
        let rect = Rect::from_xywh(constant(&self.x), constant(&self.y), w, h);
        if let Some(radius) = &self.corner_radius {
            return radius.at(0, 0.).path(rect);
        }
        let rx = self
            .rx
            .as_ref()
            .or(self.ry.as_ref())
            .map_or(0., constant)
            .min(w * 0.5);
        let ry = self
            .ry
            .as_ref()
            .or(self.rx.as_ref())
            .map_or(0., constant)
            .min(h * 0.5);
        if rx == 0. || ry == 0. {
            SkPath::rect(rect, None)
        } else {
            SkPath::rrect(RRect::new_rect_xy(rect, rx, ry), None)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRect {
    #[serde(flatten)]
    pub geometry: RectGeometry,
    #[serde(default = "default_fill")]
    pub fill: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
}

impl NativeRect {
    pub(crate) fn mask(geometry: RectGeometry) -> Self {
        Self {
            geometry,
            fill: Some([255; 4]),
            stroke: None,
        }
    }

    pub(crate) fn animated(&self) -> bool {
        self.geometry.animated() || self.stroke.as_ref().is_some_and(Stroke::animated)
    }

    pub(crate) fn sampled(&self, frame: u32, fraction: f64) -> Self {
        Self {
            geometry: self.geometry.sampled(frame, fraction),
            fill: self.fill,
            stroke: self.stroke.as_ref().map(|s| s.sampled(frame, fraction)),
        }
    }

    pub(crate) fn paint(&self) -> Result<Raster> {
        paint_prepared(self.prepare()?)
    }

    pub(crate) fn paint_on<P: super::skia_gpu::Painter>(
        &self,
        painter: &mut P,
    ) -> Result<P::Picture> {
        super::path::paint_prepared_on(self.prepare()?, painter)
    }

    #[cfg(feature = "host-audio")]
    pub(crate) fn paint_bounds(&self) -> Result<([i32; 2], [u32; 2])> {
        super::path::prepared_bounds(&self.prepare()?)
    }

    fn prepare(&self) -> Result<super::path::PreparedPath> {
        ensure!(!self.animated(), "rectangle requires a local sample time");
        prepare_geometry(
            self.geometry.path(),
            None,
            FillRule::Nonzero,
            self.fill,
            self.stroke.as_ref(),
        )
    }

    /// Property timing stays inside each key's interval. Independent extrema
    /// give a conservative swept envelope for effect workspace admission.
    pub(crate) fn raster_bounds(&self) -> Result<([i32; 2], [u32; 2])> {
        Ok(self.raster_envelope()?.swept)
    }

    pub(crate) fn raster_envelope(&self) -> Result<super::RasterEnvelope> {
        let geom = &self.geometry;
        if let Some(radius) = &geom.corner_radius {
            ensure!(
                geom.rx.is_none() && geom.ry.is_none(),
                "corner-radius and SVG rectangle axes cannot both own the corners"
            );
            radius.validate()?;
        }
        for (name, radius) in [("rx", &geom.rx), ("ry", &geom.ry)] {
            if let Some(radius) = radius {
                ensure!(
                    radius.range(name)?[0] >= 0.,
                    "rectangle {name} must be nonnegative"
                );
            }
        }
        let stroke = self.stroke.as_ref().map_or(Ok(0.), Stroke::max_width)?;
        ensure!(
            stroke <= MAX_SURFACE_DIMENSION as f32,
            "rectangle stroke exceeds surface limit"
        );
        let pad = f64::from(stroke) / 2. + 1.;
        let axis = |position: &RectLength,
                    pos_name,
                    extent: &RectLength,
                    size_name|
         -> Result<(i32, u32, u32)> {
            let [lo, hi] = position.range(pos_name)?;
            let [small, large] = extent.range(size_name)?;
            ensure!(small >= 0., "rectangle {size_name} must be nonnegative");
            // Floor/ceil of two fractional edges can occupy one extra pixel.
            let max_size = (f64::from(large) + 2. * pad).ceil() + 1.;
            ensure!(
                max_size <= f64::from(MAX_SURFACE_DIMENSION),
                "rectangle {size_name} exceeds surface limit"
            );
            let left = (f64::from(lo) - pad).floor();
            let right = (f64::from(hi + large) + pad).ceil();
            ensure!(
                left >= f64::from(i32::MIN) && right <= f64::from(i32::MAX),
                "rectangle {pos_name} outside coordinate range"
            );
            Ok((left as i32, (right - left) as u32, max_size as u32))
        };
        let (x, w, max_w) = axis(&geom.x, "x", &geom.width, "width")?;
        let (y, h, max_h) = axis(&geom.y, "y", &geom.height, "height")?;
        Ok(super::RasterEnvelope {
            swept: ([x, y], [w, h]),
            max_size: [max_w.min(w), max_h.min(h)],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn corner_radius_fits_proportionally_after_fractional_sampling() -> Result<()> {
        let rect = |radius| -> Result<NativeRect> {
            Ok(serde_json::from_value(json!({
                "width":120, "height":40, "corner_radius":radius
            }))?)
        };
        let paint = |value| rect(value)?.sampled(0, 0.).paint();
        // The left edge limits this shape. All four values use its factor.
        assert_eq!(
            paint(json!([60, 40, 30, 20]))?.rgba,
            paint(json!([30, 20, 15, 10]))?.rgba
        );
        assert_eq!(paint(json!(100))?.rgba, paint(json!(20))?.rgba);
        let pill = rect(json!([{"frame":0,"value":100},{"frame":2,"value":0}]))?;
        // Halfway through the authored 100→0 animation the requested radius
        // is still 50. Fitting the keys before interpolation would yield 10.
        assert_eq!(pill.sampled(1, 0.).paint()?.rgba, paint(json!(20))?.rgba);
        assert_eq!(
            serde_json::to_value(&pill)?["corner_radius"][0]["value"],
            100.
        );
        let animated = rect(json!([
            {"frame":0,"value":24},
            {"frame":3,"value":[24,24,24,0],"ease":"hold"},
            {"frame":5,"value":0}
        ]))?;
        animated.raster_bounds()?;
        assert_eq!(
            animated.sampled(1, 0.5).paint()?.rgba,
            paint(json!([24, 24, 24, 12]))?.rgba
        );
        assert_eq!(
            animated.sampled(4, 0.75).paint()?.rgba,
            paint(json!([24, 24, 24, 0]))?.rgba
        );
        assert_eq!(animated.sampled(6, 0.).paint()?.rgba, paint(json!(0))?.rgba);
        let eased = rect(json!([
            {"frame":0,"value":0,"ease":{"cubic":[0,1,0,1]}},
            {"frame":8,"value":[8,0,8,0]}
        ]))?;
        assert_eq!(
            eased.sampled(1, 0.).paint()?.rgba,
            paint(json!([7, 0, 7, 0]))?.rgba
        );
        // Read one pixel inside each of the four corners in clockwise order.
        let corners = [(0, 0), (119, 0), (119, 39), (0, 39)];
        for large_corner in 0..4 {
            let mut values = [0; 4];
            values[large_corner] = 15;
            let image = paint(json!(values))?;
            for (i, (x, y)) in corners.into_iter().enumerate() {
                let x = (x - image.origin[0]) as usize;
                let y = (y - image.origin[1]) as usize;
                let alpha = image.rgba[(y * image.size[0] as usize + x) * 4 + 3];
                assert_eq!(alpha, if i == large_corner { 0 } else { 255 });
            }
        }
        for bad in [
            json!(-1),
            json!([3, -1, 3, 3]),
            json!([]),
            json!([{"frame":0,"value":-1}]),
        ] {
            assert!(rect(bad)?.raster_bounds().is_err());
        }
        for bad in [json!([1, 2]), json!([1, 2, 3]), json!([1, 2, 3, 4, 5])] {
            assert!(rect(bad).is_err());
        }
        let mut mixed = rect(json!(3))?;
        mixed.geometry.rx = Some(RectLength::default());
        assert!(mixed.raster_bounds().is_err());
        Ok(())
    }

    #[test]
    fn rectangle_lengths_follow_svg_radii_and_preserve_raw_keys() -> Result<()> {
        let data = json!({
            "x": 0.5, "width": [{"frame":0,"value":8}, {"frame":2,"value":32}],
            "height": 20, "ry": 12, "stroke":{"width":1,"rgba":[255,0,0,255]}
        });
        let authored: NativeRect = serde_json::from_value(data)?;
        authored.raster_bounds()?;
        let sampled = authored.sampled(0, 0.5);
        let elliptical: NativeRect = serde_json::from_value(json!({
            "x":0.5, "width":14, "height":20, "rx":7, "ry":10,
            "stroke":{"width":1,"rgba":[255,0,0,255]}
        }))?;
        let a = sampled.paint()?;
        let b = elliptical.paint()?;
        assert_eq!((a.origin, a.size, a.rgba), (b.origin, b.size, b.rgba));
        assert_eq!(serde_json::to_value(&authored)?["ry"], 12.);
        assert!(serde_json::to_value(&authored)?["width"].is_array());
        let rect = |rx, ry, w| -> Result<NativeRect> {
            Ok(serde_json::from_value(
                json!({"width":w,"height":14,"rx":rx,"ry":ry}),
            )?)
        };
        assert_eq!(rect(0, 0, 18)?.paint()?.rgba, rect(5, 0, 18)?.paint()?.rgba);
        let mut degenerate = rect(2, 2, 0)?;
        degenerate.stroke = authored.stroke.clone();
        let empty = degenerate.paint()?;
        assert_eq!((empty.size, empty.rgba), ([1, 1], vec![0; 4]));
        for bad in [
            json!({"width":-1,"height":8}),
            json!({"width":8,"height":9,"rx":-1}),
            json!({"width":[],"height":1}),
            json!({"width":1,"height":[{"frame":2,"value":1},{"frame":1,"value":2}]}),
            json!({"width":2,"height":3,"x":1e30}),
        ] {
            assert!(
                serde_json::from_value::<NativeRect>(bad)?
                    .raster_bounds()
                    .is_err()
            );
        }
        assert!(
            serde_json::from_value::<NativeRect>(json!({"width":8,"height":9,"centre":3})).is_err()
        );
        Ok(())
    }
}
