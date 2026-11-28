//! Native vector geometry. The cached image carries the location of its first
//! texel in the authored path's coordinate system.
use crate::{
    MAX_SURFACE_DIMENSION,
    graphics::Raster,
    render::channels::{AnimatedScalar, Scalar},
    render::curve::{CurveEase, CurvePoint, PathData},
    render::skia_gpu::{Cpu, Painter},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use skia_safe::{
    AlphaType, Canvas, Color, ColorType, ContourMeasure, ContourMeasureIter, ImageInfo, Paint,
    Path as SkPath, PathBuilder, PathFillType, Rect, paint, path_utils, surfaces,
};
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativePath {
    pub d: PathData,
    #[serde(default)]
    pub fill_rule: FillRule,
    #[serde(default = "default_fill")]
    pub fill: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_start: Option<AnimatedScalar<Scalar>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_end: Option<AnimatedScalar<Scalar>>,
}

pub(super) fn default_fill() -> Option<[u8; 4]> {
    Some([0, 0, 0, 255])
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FillRule {
    #[default]
    Nonzero,
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub rgba: [u8; 4],
    pub width: AnimatedScalar<StrokeWidth>,
    #[serde(default)]
    pub cap: StrokeCap,
    #[serde(default)]
    pub join: StrokeJoin,
}

impl Stroke {
    pub(crate) fn animated(&self) -> bool {
        matches!(self.width, AnimatedScalar::Keys(_))
    }

    pub(super) fn sampled(&self, frame: u32, fraction: f64) -> Self {
        Self {
            width: self.width.sampled(frame, fraction),
            ..*self
        }
    }

    pub(super) fn max_width(&self) -> Result<f32> {
        Ok(self.width.range("stroke-width")?[1])
    }

    fn paint(&self) -> Result<Option<Paint>> {
        let AnimatedScalar::Value(width) = self.width else {
            anyhow::bail!("stroke-width animation needs a local sample time")
        };
        if width.0 == 0. {
            return Ok(None);
        }
        ensure!(
            width.0 <= MAX_SURFACE_DIMENSION as f32,
            "path stroke exceeds surface limit"
        );
        let mut brush = solid(self.rgba);
        set_stroke(&mut brush, width.0, self.cap, self.join);
        Ok(Some(brush))
    }
}

impl NativePath {
    pub(crate) fn trim_animated(&self) -> bool {
        matches!(self.stroke_start, Some(AnimatedScalar::Keys(_)))
            || matches!(self.stroke_end, Some(AnimatedScalar::Keys(_)))
    }

    pub(crate) fn animated(&self) -> bool {
        matches!(self.d, PathData::Animated(_))
            || self.stroke.as_ref().is_some_and(Stroke::animated)
            || self.trim_animated()
    }

    fn validate_trim(&self) -> Result<()> {
        for (name, property) in [
            ("stroke-start", &self.stroke_start),
            ("stroke-end", &self.stroke_end),
        ] {
            if let Some(property) = property {
                ensure!(self.stroke.is_some(), "{name} requires a stroke");
                let [lo, hi] = property.range(name)?;
                ensure!(lo >= 0. && hi <= 1., "{name} must be between 0 and 1");
            }
        }
        Ok(())
    }

    fn interval(&self) -> Result<[f32; 2]> {
        ensure!(
            !self.trim_animated(),
            "stroke trim needs a local sample time"
        );
        let read = |property: &Option<AnimatedScalar<Scalar>>, default| {
            property
                .as_ref()
                .map_or(default, |p| f32::from(p.at(0, 0.)))
        };
        Ok([read(&self.stroke_start, 0.), read(&self.stroke_end, 1.)])
    }

    pub(crate) fn sampled(&self, frame: u32, fraction: f64) -> Self {
        Self {
            d: match &self.d {
                PathData::Animated(motion) => PathData::Cubic(motion.sample(frame, fraction)),
                d => d.clone(),
            },
            stroke: self.stroke.as_ref().map(|s| s.sampled(frame, fraction)),
            stroke_start: self
                .stroke_start
                .as_ref()
                .map(|v| v.sampled(frame, fraction)),
            stroke_end: self.stroke_end.as_ref().map(|v| v.sampled(frame, fraction)),
            ..*self
        }
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrokeJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// A finite, nonnegative width. Canonical zero and finite values make equality
/// and hashing consistent for fractional widths in the artwork cache.
#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct StrokeWidth(f32);

impl TryFrom<f32> for StrokeWidth {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self> {
        ensure!(
            value.is_finite() && value >= 0.,
            "invalid path stroke width"
        );
        Ok(Self(if value == 0. { 0. } else { value }))
    }
}

impl From<StrokeWidth> for f32 {
    fn from(width: StrokeWidth) -> Self {
        width.0
    }
}

impl Eq for StrokeWidth {}
impl Hash for StrokeWidth {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state)
    }
}

pub(super) fn parse_geometry(d: &str) -> Result<SkPath> {
    // The binding converts the string to CString internally.
    ensure!(!d.as_bytes().contains(&0), "NUL in path data");
    let path = SkPath::from_svg(d).context("invalid SVG path data")?;
    ensure!(
        path.is_finite() && path.is_valid(),
        "nonfinite or invalid path geometry"
    );
    Ok(path)
}

pub(super) fn set_stroke(brush: &mut Paint, width: f32, cap: StrokeCap, join: StrokeJoin) {
    brush
        .set_style(paint::Style::Stroke)
        .set_stroke_width(width);
    brush.set_stroke_cap(match cap {
        StrokeCap::Butt => paint::Cap::Butt,
        StrokeCap::Round => paint::Cap::Round,
        StrokeCap::Square => paint::Cap::Square,
    });
    brush.set_stroke_join(match join {
        StrokeJoin::Miter => paint::Join::Miter,
        StrokeJoin::Round => paint::Join::Round,
        StrokeJoin::Bevel => paint::Join::Bevel,
    });
}

/// Size and origin have been validated by the caller. The canvas is in local units.
pub(super) fn rasterize(
    size: [u32; 2],
    origin: [i32; 2],
    draw: impl FnOnce(&Canvas) -> Result<()>,
) -> Result<Raster> {
    let [w, h] = size;
    let info = ImageInfo::new(
        (w as i32, h as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let mut rgba = super::transparent_pixels(info.compute_min_byte_size())
        .context("allocate vector surface")?;
    {
        let mut surface =
            surfaces::wrap_pixels(&info, &mut rgba, None, None).context("create vector surface")?;
        let canvas = surface.canvas();
        canvas.translate((-(origin[0] as f32), -(origin[1] as f32)));
        draw(canvas)?;
    }
    Ok(Raster { origin, size, rgba })
}

pub(super) struct PreparedPath {
    geometry: SkPath,
    stroke_geometry: Option<SkPath>,
    fill: Option<Paint>,
    stroke: Option<Paint>,
    bounds: Rect,
}

fn solid([r, g, b, a]: [u8; 4]) -> Paint {
    let mut brush = Paint::default();
    brush
        .set_color(Color::from_argb(a, r, g, b))
        .set_anti_alias(true);
    brush
}

fn geometry(data: &PathData) -> Result<SkPath> {
    match data {
        PathData::Svg(d) => parse_geometry(d),
        PathData::Cubic(contour) => contour.geometry(),
        PathData::Animated(_) => anyhow::bail!("path motion needs a local sample time"),
    }
}

/// Distances accumulate in drawing order. Each move begins a new contour.
pub(crate) struct MeasuredPath {
    source: SkPath,
    contours: Vec<ContourMeasure>,
    length: f64,
}

impl MeasuredPath {
    pub(crate) fn new(d: &PathData) -> Result<Self> {
        Self::from_geometry(geometry(d)?)
    }

    fn from_geometry(source: SkPath) -> Result<Self> {
        geometry_bounds(&source)?;
        let contours: Vec<_> = ContourMeasureIter::new(&source, false, None).collect();
        let length = contours.iter().map(|c| f64::from(c.length())).sum::<f64>();
        ensure!(length.is_finite(), "nonfinite measured path length");
        Ok(Self {
            source,
            contours,
            length,
        })
    }

    fn segment(&self, [start, end]: [f32; 2]) -> Result<SkPath> {
        let start = f64::from(start) * self.length;
        let end = f64::from(end) * self.length;
        let mut offset = 0.;
        let mut builder = PathBuilder::new();
        for contour in &self.contours {
            let length = contour.length();
            let next = offset + f64::from(length);
            if end <= offset {
                break;
            }
            if start < next {
                let a = (start - offset).max(0.) as f32;
                let b = (end - offset).min(f64::from(length)) as f32;
                if a < b {
                    ensure!(
                        contour.get_segment(a, b, &mut builder, true),
                        "could not extract stroke interval"
                    );
                    if a == 0. && b == length && contour.is_closed() {
                        builder.close();
                    }
                }
            }
            offset = next;
        }
        Ok(builder.detach())
    }
}

fn geometry_bounds(geometry: &SkPath) -> Result<Rect> {
    let raw = geometry.compute_tight_bounds();
    ensure!(raw.is_finite(), "invalid path bounds");
    let max = MAX_SURFACE_DIMENSION as f32;
    ensure!(
        raw.width() <= max && raw.height() <= max,
        "path exceeds surface limit"
    );
    Ok(raw)
}

pub(super) fn prepare_geometry(
    mut geometry: SkPath,
    stroke_geometry: Option<SkPath>,
    fill_rule: FillRule,
    color: Option<[u8; 4]>,
    stroke: Option<&Stroke>,
) -> Result<PreparedPath> {
    geometry.set_fill_type(match fill_rule {
        FillRule::Nonzero => PathFillType::Winding,
        FillRule::EvenOdd => PathFillType::EvenOdd,
    });
    let raw = geometry_bounds(&geometry)?;
    let fill = color.map(solid);
    let mut bounds = if fill.is_some() { raw } else { Rect::default() };
    let stroke = stroke.map(Stroke::paint).transpose()?.flatten();
    let stroke_path = stroke_geometry.as_ref().unwrap_or(&geometry);
    if let Some(brush) = &stroke
        && !stroke_path.is_empty()
    {
        let mut outline = PathBuilder::new();
        ensure!(
            path_utils::fill_path_with_paint(stroke_path, brush, &mut outline, None, None),
            "could not bound path stroke"
        );
        let outline = outline.detach();
        ensure!(outline.is_finite(), "invalid stroked path geometry");
        bounds.join(outline.compute_tight_bounds());
    }
    Ok(PreparedPath {
        geometry,
        stroke_geometry,
        fill,
        stroke,
        bounds,
    })
}

fn raster_box(bounds: Rect) -> Result<([i32; 2], [u32; 2])> {
    // A transparent pixel is a valid wgpu texture for empty or unpainted geometry.
    if bounds.is_empty() {
        return Ok(([0, 0], [1, 1]));
    }
    let axis = |lo: f32, hi: f32| -> Result<(i32, u32)> {
        // Reserve a pixel beyond the mathematical edge for antialiasing.
        let lo = f64::from(lo).floor() - 1.;
        let hi = f64::from(hi).ceil() + 1.;
        ensure!(
            lo >= f64::from(i32::MIN) && hi <= f64::from(i32::MAX),
            "path bounds outside coordinate range"
        );
        let length = hi - lo;
        ensure!(
            (1. ..=f64::from(MAX_SURFACE_DIMENSION)).contains(&length),
            "painted path exceeds surface limit"
        );
        Ok((lo as i32, length as u32))
    };
    let (x, w) = axis(bounds.left, bounds.right)?;
    let (y, h) = axis(bounds.top, bounds.bottom)?;
    Ok(([x, y], [w, h]))
}

pub(crate) fn raster_bounds(path: &NativePath) -> Result<([i32; 2], [u32; 2])> {
    Ok(raster_envelope(path)?.swept)
}

pub(super) fn raster_envelope(path: &NativePath) -> Result<super::RasterEnvelope> {
    path.validate_trim()?;
    let max_width = path.stroke.as_ref().map_or(Ok(0.), Stroke::max_width)?;
    ensure!(
        max_width <= MAX_SURFACE_DIMENSION as f32,
        "path stroke exceeds surface limit"
    );
    if let PathData::Animated(motion) = &path.d {
        motion.validate()?;
        // A cubic, in either space or time, lies inside its control hull. A miter
        // can reach four half-widths and a square cap reaches sqrt(2) radii.
        let pad = path.stroke.as_ref().map_or(0., |s| {
            let extent = if s.join == StrokeJoin::Miter {
                4.
            } else if s.cap == StrokeCap::Square {
                2f64.sqrt()
            } else {
                1.
            };
            f64::from(max_width) * 0.5 * extent
        }) + 1.;
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        let mut max_size = [0; 2];
        let mut include_pose = |points: &[CurvePoint]| -> Result<()> {
            let mut pose_low = [f64::INFINITY; 2];
            let mut pose_high = [f64::NEG_INFINITY; 2];
            for point in points {
                let xy: [f32; 2] = (*point).into();
                for axis in 0..2 {
                    let coord = f64::from(xy[axis]);
                    pose_low[axis] = pose_low[axis].min(coord);
                    pose_high[axis] = pose_high[axis].max(coord);
                }
            }
            for axis in 0..2 {
                let span = (pose_high[axis] - pose_low[axis] + 2. * pad).ceil() + 1.;
                ensure!(
                    span <= f64::from(MAX_SURFACE_DIMENSION),
                    "animated path exceeds surface limit"
                );
                max_size[axis] = max_size[axis].max(span as u32);
                low[axis] = low[axis].min((pose_low[axis] - pad).floor());
                high[axis] = high[axis].max((pose_high[axis] + pad).ceil());
                ensure!(
                    low[axis] >= f64::from(i32::MIN) && high[axis] <= f64::from(i32::MAX),
                    "animated path outside coordinate range"
                );
            }
            Ok(())
        };
        for (i, key) in motion.keys.iter().enumerate() {
            include_pose(&key.points)?;
            if key.ease == CurveEase::Spline && i + 1 < motion.keys.len() {
                let (outgoing, incoming): (Vec<_>, Vec<_>) =
                    motion.spline_controls(i).map(|[_, a, b, _]| (a, b)).unzip();
                include_pose(&outgoing)?;
                include_pose(&incoming)?;
            }
        }
        let size = std::array::from_fn(|i| (high[i] - low[i]) as u32);
        return Ok(super::RasterEnvelope {
            swept: (low.map(|v| v as i32), size),
            max_size: std::array::from_fn(|i| max_size[i].min(size[i])),
        });
    }
    // A new interior cap may extend outside the full stroke at a bend.
    let widest = path.stroke.as_ref().map(|s| Stroke {
        width: AnimatedScalar::Value(StrokeWidth(max_width)),
        ..*s
    });
    if !path.trim_animated() {
        return raster_box(prepare_path(path, widest.as_ref(), None)?.bounds)
            .map(super::RasterEnvelope::fixed);
    }
    let source = geometry(&path.d)?;
    let mut bounds = prepare_geometry(
        source.clone(),
        None,
        path.fill_rule,
        path.fill,
        widest.as_ref(),
    )?
    .bounds;
    if max_width > 0. {
        let radius = if widest.as_ref().is_some_and(|s| s.cap == StrokeCap::Square) {
            max_width * 0.5 * std::f32::consts::SQRT_2
        } else {
            max_width * 0.5
        };
        bounds.join(source.compute_tight_bounds().with_outset((radius, radius)));
    }
    raster_box(bounds).map(super::RasterEnvelope::fixed)
}

pub(crate) fn validate_path(path: &NativePath) -> Result<()> {
    raster_bounds(path).map(|_| ())
}

pub(crate) fn paint_path(path: &NativePath) -> Result<Raster> {
    paint_with_measure(path, None)
}

fn prepare_path(
    path: &NativePath,
    stroke: Option<&Stroke>,
    measured: Option<&MeasuredPath>,
) -> Result<PreparedPath> {
    let interval = path.interval()?;
    let source = match measured {
        Some(m) => m.source.clone(),
        None => geometry(&path.d)?,
    };
    let trimmed = if interval == [0., 1.] {
        None
    } else if interval[0] >= interval[1] {
        Some(SkPath::new())
    } else {
        Some(match measured {
            Some(m) => m.segment(interval)?,
            None => MeasuredPath::from_geometry(source.clone())?.segment(interval)?,
        })
    };
    prepare_geometry(source, trimmed, path.fill_rule, path.fill, stroke)
}

pub(crate) fn paint_with_measure(
    path: &NativePath,
    measured: Option<&MeasuredPath>,
) -> Result<Raster> {
    paint_with_measure_on(path, measured, &mut Cpu)
}

pub(crate) fn paint_with_measure_on<P: Painter>(
    path: &NativePath,
    measured: Option<&MeasuredPath>,
    painter: &mut P,
) -> Result<P::Picture> {
    paint_prepared_on(prepare_path(path, path.stroke.as_ref(), measured)?, painter)
}

#[cfg(feature = "host-audio")]
pub(super) fn prepared_bounds(p: &PreparedPath) -> Result<([i32; 2], [u32; 2])> {
    raster_box(p.bounds)
}

pub(super) fn paint_prepared(p: PreparedPath) -> Result<Raster> {
    paint_prepared_on(p, &mut Cpu)
}

pub(super) fn paint_prepared_on<P: Painter>(
    p: PreparedPath,
    painter: &mut P,
) -> Result<P::Picture> {
    let (origin, size) = raster_box(p.bounds)?;
    painter.paint(size, origin, |canvas| {
        if let Some(fill) = p.fill {
            canvas.draw_path(&p.geometry, &fill);
        }
        if let Some(stroke) = p.stroke {
            canvas.draw_path(p.stroke_geometry.as_ref().unwrap_or(&p.geometry), &stroke);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(d: &str) -> NativePath {
        NativePath {
            d: d.into(),
            fill_rule: FillRule::Nonzero,
            fill: Some([255, 255, 255, 255]),
            stroke_start: None,
            stroke_end: None,
            stroke: None,
        }
    }

    fn pixel(r: &Raster, x: i32, y: i32) -> [u8; 4] {
        let x = x - r.origin[0];
        let y = y - r.origin[1];
        if x < 0 || y < 0 || x as u32 >= r.size[0] || y as u32 >= r.size[1] {
            return [0; 4];
        }
        let i = ((y as usize * r.size[0] as usize) + x as usize) * 4;
        r.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn local_coverage_and_winding_survive_tight_bounds() -> Result<()> {
        let mut p = shape("M-16 -16 H16 V16 H-16 Z M-8 -8 H8 V8 H-8 Z");
        assert_eq!(pixel(&paint_path(&p)?, 0, 0), [255; 4]);
        p.fill_rule = FillRule::EvenOdd;
        let r = paint_path(&p)?;
        assert_eq!(pixel(&r, 0, 0), [0; 4]);
        assert_eq!(pixel(&r, -12, -12), [255; 4]);
        assert_eq!(pixel(&r, -20, -12), [0; 4]);
        let r = paint_path(&shape("M0.5 0 H8 V8 H0.5 Z"))?;
        let edge = pixel(&r, 0, 4);
        assert!((126..=129).contains(&edge[3]) && edge == [edge[3]; 4]);
        Ok(())
    }

    #[test]
    fn curve_with_fractional_stroke_matches_svg_and_preserves_caps_and_joins() -> Result<()> {
        use std::fs;
        let d = "M4 12 C6 4 14 3 20 12 L27 7 L29 13";
        let mut p = shape(d);
        p.fill = Some([50, 105, 149, 255]);
        p.stroke = Some(Stroke {
            rgba: [0, 255, 0, 255],
            width: 5.5f32.try_into()?,
            cap: StrokeCap::Square,
            join: StrokeJoin::Miter,
        });
        let r = paint_path(&p)?;
        let dir = tempfile::tempdir()?;
        let svg = dir.path().join("reference.svg");
        fs::write(
            &svg,
            format!(
                r##"<svg width="40" height="30"><path d="{d}" fill="#326995" stroke="#00ff00" stroke-width="5.5" stroke-linecap="square" stroke-linejoin="miter"/></svg>"##
            ),
        )?;
        let reference = super::super::svg::paint_svg(&svg)?;
        for y in 0..30 {
            for x in 0..40 {
                assert_eq!(pixel(&r, x, y), pixel(&reference, x, y), "at {x},{y}");
            }
        }
        p.d = "M-4 0 H8".into();
        p.fill = None;
        let r = paint_path(&p)?;
        assert_eq!(pixel(&r, -6, 0), [0, 255, 0, 255]);
        assert_eq!(pixel(&r, 9, 0), [0, 255, 0, 255]);
        assert_eq!(pixel(&r, -8, 0), [0; 4]);
        Ok(())
    }

    #[test]
    fn width_and_contour_share_the_fractional_sample_and_fit_the_validated_envelope() -> Result<()>
    {
        use serde_json::json;
        let mut path: NativePath = serde_json::from_value(json!({
            "d":{"keys":[
                {"frame":0,"points":[[0,0],[2,0],[6,0],[8,0]]},
                {"frame":4,"points":[[8,0],[10,0],[14,0],[16,0]]}
            ]},
            "fill":null, "stroke":{"rgba":[255,255,255,255],"cap":"square",
                "width":[{"frame":0,"value":0,"ease":{"cubic":[0,1,0,1]}},{"frame":4,"value":8}]}
        }))?;
        let (origin, size) = raster_bounds(&path)?;
        let sample = paint_path(&path.sampled(0, 0.5))?;
        // At 1/8 elapsed the centerline moves 1 px and the eased width is 7 px.
        let mut reference = shape("M1 0 H9");
        reference.fill = None;
        reference.stroke =
            serde_json::from_value(json!({"rgba":[255,255,255,255],"cap":"square","width":7}))?;
        let expected = paint_path(&reference)?;
        assert_eq!(
            (sample.origin, sample.size, sample.rgba),
            (expected.origin, expected.size, expected.rgba)
        );
        for frame in 0..=4 {
            let r = paint_path(&path.sampled(frame, 0.))?;
            for axis in 0..2 {
                assert!(r.origin[axis] >= origin[axis]);
                assert!(
                    i64::from(r.origin[axis]) + i64::from(r.size[axis])
                        <= i64::from(origin[axis]) + i64::from(size[axis])
                );
            }
        }
        path.d = "M1 0H9".into();
        assert!(paint_path(&path).is_err());
        let widest = paint_path(&path.sampled(4, 0.))?;
        assert_eq!(raster_bounds(&path)?, (widest.origin, widest.size));
        Ok(())
    }

    #[test]
    fn invalid_width_curves_are_rejected_for_rectangles_and_paths() {
        use super::super::rect::NativeRect;
        use serde_json::json;
        for width in [
            json!([]),
            json!([{"frame":0,"value":-0.25}]),
            json!([{"frame":1,"value":0},{"frame":1,"value":2}]),
            json!([{"frame":0,"value":0,"ease":{"cubic":[0,-1,1,1]}}]),
            json!([{"frame":0,"value":1},{"frame":30,"value":8193}]),
        ] {
            let stroke = json!({"rgba":[0,0,0,255],"width":width});
            let path = json!({"d":"M0 0H4","stroke":stroke});
            let rect = json!({"width":4,"height":4,"stroke":stroke});
            assert!(
                serde_json::from_value::<NativePath>(path)
                    .map_err(anyhow::Error::from)
                    .and_then(|p| validate_path(&p))
                    .is_err(),
                "path accepted {width}"
            );
            assert!(
                serde_json::from_value::<NativeRect>(rect)
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r.raster_bounds())
                    .is_err(),
                "rect accepted {width}"
            );
        }
    }

    #[test]
    fn trim_measures_ink_across_moves_and_keeps_a_fully_selected_closing_join() -> Result<()> {
        let ink = |d: &str| -> Result<NativePath> {
            Ok(serde_json::from_value(serde_json::json!({
                "d": d, "fill": null, "stroke":{"rgba":[230,190,80,255],"width":3.5,
                    "cap":"butt", "join":"miter"}
            }))?)
        };
        // The square and open line are each 32 units. The solitary move/zero
        // length line contributes no ink or travel time to this interval.
        let mut p = ink("M0 0H8V8H0Z M19 5l0 0 M24 4h32")?;
        p.stroke_end = Some(0.75.try_into()?);
        let r = paint_path(&p)?;
        let expected = paint_path(&ink("M0 0H8V8H0Z M24 4h16")?)?;
        assert_eq!(
            (r.origin, r.size, r.rgba),
            (expected.origin, expected.size, expected.rgba)
        );
        p.stroke_start = Some(0.375.try_into()?);
        let r = paint_path(&p)?;
        let expected = paint_path(&ink("M0 8V0 M24 4h16")?)?;
        assert_eq!(
            (r.origin, r.size, r.rgba),
            (expected.origin, expected.size, expected.rgba)
        );
        Ok(())
    }

    #[test]
    fn empty_trim_keeps_fill_and_has_no_round_cap_dot() -> Result<()> {
        let mut p = shape("M0 0H12V9Z");
        let expected = paint_path(&p)?;
        p.stroke = serde_json::from_value(
            serde_json::json!({"rgba":[10,30,90,255],"width":6,"cap":"round"}),
        )?;
        p.stroke_end = Some(0.25.try_into()?);
        for start in [0.25, 0.8] {
            p.stroke_start = Some(start.try_into()?);
            let r = paint_path(&p)?;
            assert_eq!(
                (r.origin, r.size, &r.rgba),
                (expected.origin, expected.size, &expected.rgba)
            );
        }
        Ok(())
    }

    #[test]
    fn invalid_trim_tracks_fail_before_sampling() -> Result<()> {
        use serde_json::json;
        for tracks in [
            json!({"stroke_start":-0.1}),
            json!({"stroke_end":[{"frame":0,"value":1.1}]}),
            json!({"stroke_start":[]}),
            json!({"stroke_end":[{"frame":4,"value":0},{"frame":3,"value":1}]}),
        ] {
            let mut encoded = json!({"d":"M0 0H10","stroke":{"width":2,"rgba":[255,255,255,255]}});
            encoded
                .as_object_mut()
                .unwrap()
                .extend(tracks.as_object().unwrap().clone());
            let path = serde_json::from_value(encoded)?;
            assert!(validate_path(&path).is_err(), "admitted {tracks}");
        }
        let mut unstroked = shape("M0 0H10");
        unstroked.stroke_end = Some(0.6.try_into()?);
        assert!(validate_path(&unstroked).is_err());
        Ok(())
    }

    #[test]
    fn trim_uses_distance_on_a_nonuniform_cubic_and_the_sampled_centerline() -> Result<()> {
        // The cubic's parameter midpoint is x=12.5. Its distance midpoint is x=50.
        let measure = MeasuredPath::new(&"M0 0 C0 0 0 0 100 0".into())?;
        assert!((measure.segment([0., 0.5])?.compute_tight_bounds().right - 50.).abs() < 0.25);
        let p: NativePath = serde_json::from_value(serde_json::json!({
            "d":{"keys":[
                {"frame":0,"points":[[0,0],[8,0],[16,0],[24,0]]},
                {"frame":2,"points":[[0,0],[8,8],[16,16],[24,24]]}
            ]},
            "fill":null, "stroke":{"width":2,"cap":"square","rgba":[255,255,255,255]},
            "stroke_end":[{"frame":0,"value":0},{"frame":2,"value":1}]
        }))?;
        let (lo, size) = raster_bounds(&p)?;
        let sampled = paint_path(&p.sampled(1, 0.5))?;
        let mut expected = p.sampled(1, 0.5);
        expected.d = "M0 0 C6 4.5 12 9 18 13.5".into();
        expected.stroke_end = None;
        let oracle = paint_path(&expected)?;
        assert_eq!(
            (sampled.origin, sampled.size, sampled.rgba),
            (oracle.origin, oracle.size, oracle.rgba)
        );
        for i in 0..2 {
            assert!(sampled.origin[i] >= lo[i]);
            assert!(
                i64::from(sampled.origin[i]) + i64::from(sampled.size[i])
                    <= i64::from(lo[i]) + i64::from(size[i])
            );
        }
        Ok(())
    }

    #[test]
    fn invalid_geometry_and_excessive_bounds_fail_before_allocation() -> Result<()> {
        for bad in ["M0 0 L", "M0 0 \0 L1 1", "M0 0 L1e90 2", "M0 0 H9000 V8 Z"] {
            ensure!(
                validate_path(&shape(bad)).is_err(),
                "unexpectedly accepted {bad:?}"
            );
        }
        for width in [-1., f32::INFINITY, f32::NAN] {
            assert!(StrokeWidth::try_from(width).is_err());
        }
        let mut p = shape("M0 0 H10 V10 Z");
        p.fill = None;
        let empty = paint_path(&p)?;
        assert_eq!(empty.size, [1, 1]);
        assert_eq!(empty.rgba, [0; 4]);
        Ok(())
    }
}
