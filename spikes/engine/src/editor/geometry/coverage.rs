//! Drawable regions and authored clipping evaluated in the painter's coordinate spaces.
use super::GeometryClip;
use crate::{
    graphics::{
        Blend, Content, Graphic, PreparedRasterAssets, PreparedTextLayouts, Raster, SampleTime,
    },
    render::{LayerSample, affine::LayerSpace, sample_mask},
};
use anyhow::{Context, Result};
use std::{cell::RefCell, collections::VecDeque, sync::Arc};

/// Shared accepted artwork and editor-local query scratch.
#[derive(Clone, Copy)]
pub(in crate::editor) struct GeometryAssets<'a> {
    pub rasters: &'a PreparedRasterAssets,
    pub text: &'a PreparedTextLayouts,
    pub canvas: [u32; 2],
    pub native: &'a NativeCoverageCache,
}

pub(super) type Point = [f64; 2];
pub(super) type Bounds = [f64; 4];
type Quad = [Point; 4];
pub(super) type RasterBounds = ([i32; 2], [u32; 2]);

fn raster_box([x, y]: [i32; 2], [w, h]: [u32; 2]) -> Bounds {
    let [x, y, w, h] = [f64::from(x), f64::from(y), f64::from(w), f64::from(h)];
    [x, y, x + w, y + h]
}

pub(super) fn corners([l, t, r, b]: Bounds) -> Quad {
    [[l, t], [r, t], [r, b], [l, b]]
}

pub(super) fn bounds_of(points: impl IntoIterator<Item = Point>) -> Option<Bounds> {
    let mut points = points.into_iter();
    let [x, y] = points.next()?;
    Some(points.fold([x, y, x, y], |[l, t, r, b], [x, y]| {
        [l.min(x), t.min(y), r.max(x), b.max(y)]
    }))
}

pub(super) fn contains([l, t, r, b]: Bounds, [x, y]: Point, pad: f64) -> bool {
    x >= l - pad && x < r + pad && y >= t - pad && y < b + pad
}

pub(super) fn intersect_polygon(
    mut polygon: Vec<Point>,
    space: LayerSpace,
    bounds: Bounds,
) -> Vec<Point> {
    // Clip against each side in its actual local space. Intersections stay in
    // composition space so several differently transformed clips can accumulate.
    for (side, edge) in bounds
        .into_iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
    {
        let Some(&last) = polygon.last() else { break };
        let distance = |point| {
            let v = space.to_local(point)[side % 2];
            if side < 2 { v - edge } else { edge - v }
        };
        let mut previous = last;
        let mut before = distance(last);
        let mut clipped = Vec::with_capacity(polygon.len() + 1);
        for point in polygon {
            let after = distance(point);
            if (before < 0.) != (after < 0.) {
                let t = before / (before - after);
                clipped.push(std::array::from_fn(|i| {
                    previous[i] + t * (point[i] - previous[i])
                }));
            }
            if after >= 0. {
                clipped.push(point);
            }
            previous = point;
            before = after;
        }
        polygon = clipped;
    }
    polygon
}

pub(super) fn has_area(polygon: &[Point]) -> bool {
    let Some(&[x, y]) = polygon.first() else {
        return false;
    };
    // Translate the origin first to keep the area stable for distant artwork.
    polygon[1..]
        .windows(2)
        .any(|p| (p[0][0] - x) * (p[1][1] - y) != (p[0][1] - y) * (p[1][0] - x))
}

fn artwork_bounds(
    content: &Content,
    rasters: &PreparedRasterAssets,
) -> Result<Option<RasterBounds>> {
    Ok(match content {
        Content::Path(path) => Some(crate::render::path::raster_bounds(path)?),
        Content::Rect(rect) => Some(rect.paint_bounds()?),
        Content::Svg { source } => rasters
            .svg(source)
            .map(|raster| (raster.origin, raster.size)),
        _ => None,
    })
}

pub(super) struct LeafGeometry {
    pub(super) raster: RasterBounds,
    region: HitRegion,
}

enum HitRegion {
    Alpha,
    Opaque,
    Text(Vec<Bounds>),
}

impl LeafGeometry {
    pub(super) fn regions(&self) -> impl Iterator<Item = Bounds> + '_ {
        let rect = raster_box(self.raster.0, self.raster.1);
        let (lines, rectangle) = match &self.region {
            HitRegion::Text(lines) => (lines.as_slice(), None),
            HitRegion::Alpha | HitRegion::Opaque => (&[][..], Some(rect)),
        };
        lines.iter().copied().chain(rectangle)
    }

    pub(super) fn bounds(&self) -> Option<Bounds> {
        bounds_of(self.regions().flat_map(corners))
    }
}

pub(super) fn evaluate_leaf(
    layer: &Graphic,
    content: &Content,
    frame: u32,
    assets: GeometryAssets<'_>,
) -> Result<Option<LeafGeometry>> {
    let media_size = match content {
        Content::Label { .. } => {
            let text = assets
                .text
                .geometry(content, &layer.text_animators, frame)?;
            return Ok(Some(LeafGeometry {
                raster: text.raster_bounds,
                region: HitRegion::Text(text.regions),
            }));
        }
        // TODO(editor): Use accepted image alpha when transparent regions
        // should pass through to the layers underneath.
        Content::Image { source } => assets.rasters.image_size(source),
        Content::Video(clip) => Some(clip.size),
        Content::Footage { .. } => Some(assets.canvas),
        // TODO(editor): Give backdrop adjustments a hit area from their
        // authored region and define their precedence over underlying layers.
        // TODO(editor): Ask the spatial renderer for depth-aware object hits.
        _ => {
            return Ok(
                artwork_bounds(content, assets.rasters)?.map(|raster| LeafGeometry {
                    raster,
                    region: HitRegion::Alpha,
                }),
            );
        }
    };
    Ok(media_size.map(|size| LeafGeometry {
        raster: ([0; 2], size),
        region: HitRegion::Opaque,
    }))
}

pub(super) fn uncertain_paint(layer: &Graphic) -> bool {
    // TODO(editor): Preserve source hit coverage for alpha-preserving effects.
    // Define coverage for blur, shadows, erasure, and custom shader outputs.
    layer.filter.is_some()
        || layer.underlay.is_some()
        || !layer.effects.is_empty()
        || matches!(&layer.content, Content::Footage { history: Some(_) })
        || layer.blend == Blend::DestinationOut
        || layer
            .mask
            .as_ref()
            .is_some_and(|mask| mask.shader.is_some())
}

struct SampledMask {
    position: [i32; 2],
    bounds: Bounds,
    content: Content,
    has_shader: bool,
}

pub(super) struct Clip {
    space: LayerSpace,
    mask: Option<SampledMask>,
    wipe_right: Option<f64>,
    empty: bool,
}

impl Clip {
    fn bounds(&self) -> Bounds {
        let [l, t, mut r, b] = self.mask.as_ref().map_or(
            [
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            ],
            |mask| mask.bounds,
        );
        if let Some(wipe) = self.wipe_right {
            r = r.min(wipe);
        }
        [l, t, r, b]
    }

    pub(super) fn constrained(&self) -> bool {
        self.mask.is_some() || self.wipe_right.is_some() || self.empty
    }

    pub(super) fn geometry(&self) -> GeometryClip {
        GeometryClip {
            local_to_composition: self.space.rows(),
            mask_bounds: self.mask.as_ref().map(|mask| mask.bounds),
            wipe_right: self.wipe_right,
            empty: self.empty,
        }
    }

    pub(super) fn shape(&self, polygon: Vec<Point>) -> Vec<Point> {
        if self.empty {
            return Vec::new();
        }
        if self.constrained() {
            return intersect_polygon(polygon, self.space, self.bounds());
        }
        polygon
    }

    pub(super) fn contains(&self, point: Point, assets: GeometryAssets<'_>) -> Result<bool> {
        let local = self.space.to_local(point);
        let taps = Taps::at(local, self.space);
        if self.empty || !contains(self.bounds(), local, taps.pad()) {
            return Ok(false);
        }
        let mask = self.mask_raster(assets)?;
        Ok(taps.any(|p| self.tap(p, mask.as_ref())))
    }

    fn mask_raster<'a>(&self, assets: GeometryAssets<'a>) -> Result<Option<CoverageRaster<'a>>> {
        let Some(mask) = self.mask.as_ref().filter(|mask| !mask.has_shader) else {
            return Ok(None);
        };
        assets.coverage_raster(&mask.content).map(Some)
    }

    fn tap(&self, p: [i64; 2], raster: Option<&CoverageRaster<'_>>) -> bool {
        if self.wipe_right.is_some_and(|edge| p[0] as f64 >= edge) {
            return false;
        }
        self.mask.as_ref().is_none_or(|mask| {
            mask.has_shader
                || raster.is_some_and(|raster| {
                    let [x, y] = mask.position.map(i64::from);
                    raster.alpha([p[0] - x, p[1] - y])
                })
        })
    }
}

pub(super) fn sample_clip(
    layer: &Graphic,
    sample: &LayerSample,
    scope_clock: u32,
    artwork: RasterBounds,
    rasters: &PreparedRasterAssets,
) -> Result<Clip> {
    let mut clip = Clip {
        space: sample.space,
        mask: None,
        wipe_right: None,
        empty: false,
    };
    if let Some(wipe) = layer.wipe {
        let width = SampleTime::at(scope_clock, None, 0).reveal(wipe, artwork.1[0]);
        clip.empty = width == 0;
        if width < artwork.1[0] {
            clip.wipe_right = Some(f64::from(artwork.0[0]) + f64::from(width));
        }
    }
    if let Some(mask) = &layer.mask {
        let content = sample_mask(mask, sample.frame, 0.).unwrap_or_else(|| mask.as_content());
        let (origin, size) =
            artwork_bounds(&content, rasters)?.context("missing accepted mask geometry")?;
        let [x, y] = mask.position.map(f64::from);
        let [l, t, r, b] = raster_box(origin, size);
        clip.mask = Some(SampledMask {
            position: mask.position,
            bounds: [l + x, t + y, r + x, b + y],
            content,
            has_shader: mask.shader.is_some(),
        });
    }
    Ok(clip)
}

/// Sampled native artwork has no external dependencies. Identical shape
/// samples can reuse exact raster alpha across gestures and revisions.
#[derive(Default)]
pub(in crate::editor) struct NativeCoverageCache {
    entries: RefCell<VecDeque<(Content, Arc<Raster>)>>,
}

impl NativeCoverageCache {
    fn raster(&self, content: &Content) -> Result<Arc<Raster>> {
        const BYTE_LIMIT: usize = 32 * 1024 * 1024;
        const ENTRY_LIMIT: usize = 48;

        let mut entries = self.entries.borrow_mut();
        if let Some(index) = entries.iter().position(|(key, _)| key == content) {
            let entry = entries.remove(index).expect("located coverage raster");
            let raster = Arc::clone(&entry.1);
            entries.push_front(entry);
            return Ok(raster);
        }
        let raster = Arc::new(match content {
            Content::Rect(rect) => rect.paint()?,
            Content::Path(path) => crate::render::path::paint_path(path)?,
            _ => anyhow::bail!("unsupported native coverage raster"),
        });
        let size = raster.rgba.len();
        if size <= BYTE_LIMIT {
            let mut bytes: usize = entries.iter().map(|(_, raster)| raster.rgba.len()).sum();
            while bytes + size > BYTE_LIMIT || entries.len() >= ENTRY_LIMIT {
                bytes -= entries
                    .pop_back()
                    .expect("coverage cache entry")
                    .1
                    .rgba
                    .len();
            }
            entries.push_front((content.clone(), Arc::clone(&raster)));
        }
        Ok(raster)
    }
}

enum CoverageRaster<'a> {
    Native(Arc<Raster>),
    Svg(&'a Raster),
}

impl CoverageRaster<'_> {
    fn alpha(&self, [x, y]: [i64; 2]) -> bool {
        let raster = match self {
            Self::Native(raster) => raster.as_ref(),
            Self::Svg(raster) => raster,
        };
        let [x, y] = [
            x - i64::from(raster.origin[0]),
            y - i64::from(raster.origin[1]),
        ];
        if !(0..i64::from(raster.size[0])).contains(&x)
            || !(0..i64::from(raster.size[1])).contains(&y)
        {
            return false;
        }
        raster.rgba[(y as usize * raster.size[0] as usize + x as usize) * 4 + 3] != 0
    }
}

impl<'a> GeometryAssets<'a> {
    fn coverage_raster(self, content: &Content) -> Result<CoverageRaster<'a>> {
        match content {
            Content::Svg { source } => Ok(CoverageRaster::Svg(
                self.rasters
                    .svg(source)
                    .context("missing accepted SVG artwork")?,
            )),
            _ => self.native.raster(content).map(CoverageRaster::Native),
        }
    }
}

#[derive(Clone, Copy)]
struct Taps {
    base: [i64; 2],
    fraction: Point,
    affine: bool,
}

impl Taps {
    fn at(local: Point, space: LayerSpace) -> Self {
        let affine = space.transform.is_some();
        let local = if affine {
            local.map(|v| v - 0.5)
        } else {
            local
        };
        let base = local.map(|v| v.floor() as i64);
        let fraction = std::array::from_fn(|i| local[i] - base[i] as f64);
        Self {
            base,
            fraction,
            affine,
        }
    }

    fn pad(self) -> f64 {
        if self.affine { 0.5 } else { 0. }
    }

    fn any(self, test: impl Fn([i64; 2]) -> bool) -> bool {
        let [x, y] = self.base;
        let [fx, fy] = self.fraction;
        let [next_x, next_y] = [x.saturating_add(1), y.saturating_add(1)];
        test([x, y])
            || self.affine
                && (fx > 0. && test([next_x, y])
                    || fy > 0. && test([x, next_y])
                    || fx > 0. && fy > 0. && test([next_x, next_y]))
    }
}

pub(super) enum Coverage {
    Outside,
    Inside,
    Unknown,
}

pub(super) fn point_coverage(
    layer: &Graphic,
    content: &Content,
    leaf: &LeafGeometry,
    clip: &Clip,
    point: Point,
    assets: GeometryAssets<'_>,
) -> Result<Coverage> {
    let local = clip.space.to_local(point);
    let taps = Taps::at(local, clip.space);
    let bounds = raster_box(leaf.raster.0, leaf.raster.1);
    let source_pad = if matches!(leaf.region, HitRegion::Opaque) {
        0.
    } else {
        taps.pad()
    };
    if clip.empty
        || !contains(clip.bounds(), local, taps.pad())
        || !contains(bounds, local, source_pad)
    {
        return Ok(Coverage::Outside);
    }
    if let HitRegion::Text(lines) = &leaf.region
        && !lines.iter().any(|&line| contains(line, local, 0.))
        && !uncertain_paint(layer)
    {
        return Ok(Coverage::Outside);
    }
    let mask = clip.mask_raster(assets)?;
    if !taps.any(|p| clip.tap(p, mask.as_ref())) {
        return Ok(Coverage::Outside);
    }
    if uncertain_paint(layer) {
        return Ok(Coverage::Unknown);
    }
    // A leaf's source and mask multiply per local tap before interpolation.
    let inside = match leaf.region {
        HitRegion::Text(_) => true,
        HitRegion::Opaque => taps.any(|p| {
            clip.tap(p, mask.as_ref())
                && contains(bounds, p.map(|coordinate| coordinate as f64), 0.)
        }),
        HitRegion::Alpha => {
            let source = assets.coverage_raster(content)?;
            taps.any(|p| clip.tap(p, mask.as_ref()) && source.alpha(p))
        }
    };
    Ok(if inside {
        Coverage::Inside
    } else {
        Coverage::Outside
    })
}
