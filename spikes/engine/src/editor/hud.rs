//! Rasterize the selection outline independently of the film picture.
use super::{LayerGeometry, Visibility};
use anyhow::{Context, Result, ensure};
use skia_safe::{
    AlphaType, Canvas, Color, ColorType, ImageInfo, Paint, PathBuilder, Rect, paint, surfaces,
};
use std::sync::Arc;

pub struct HudImage {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    /// Straight RGBA8 for a host's transparent presentation surface.
    pub rgba: Arc<Vec<u8>>,
}

type Quad = [[f64; 2]; 4];

fn outline_quad(geometry: &LayerGeometry) -> Option<Quad> {
    geometry.composition_quad.filter(|_| {
        matches!(
            geometry.visibility,
            Visibility::Visible | Visibility::Indeterminate
        )
    })
}

pub fn render_selection(
    geometries: &[LayerGeometry],
    marquee: Option<[f64; 4]>,
    hovered: Option<&LayerGeometry>,
    canvas: [u32; 2],
) -> Result<Option<HudImage>> {
    let selected: Vec<_> = geometries.iter().filter_map(outline_quad).collect();
    let hover = hovered.and_then(outline_quad);
    let rectangle = marquee.map(|[l, t, r, b]| [[l, t], [r, t], [r, b], [l, b]]);
    if selected.is_empty() && hover.is_none() && rectangle.is_none() {
        return Ok(None);
    }
    let quads = selected.iter().chain(hover.iter()).chain(rectangle.iter());
    ensure!(
        quads.clone().flatten().flatten().all(|v| v.is_finite()),
        "invalid HUD geometry"
    );
    let mut origin = [0; 2];
    let mut size = [0; 2];
    // Leave room for the outline and its antialiasing, then clip to the picture.
    for axis in 0..2 {
        let extent = quads.clone().flatten().map(|p| p[axis]);
        let low = extent.clone().fold(f64::INFINITY, f64::min);
        let high = extent.fold(f64::NEG_INFINITY, f64::max);
        let limit = f64::from(canvas[axis]);
        let start = (low.floor() - 4.).clamp(0., limit) as u32;
        let end = (high.ceil() + 4.).clamp(0., limit) as u32;
        if end <= start {
            return Ok(None);
        }
        origin[axis] = start;
        size[axis] = end - start;
    }
    let dimensions = (size[0] as i32, size[1] as i32);
    let info = ImageInfo::new(dimensions, ColorType::RGBA8888, AlphaType::Premul, None);
    let mut surface = surfaces::raster(&info, None, None).context("allocate selection outline")?;
    let raster = surface.canvas();
    raster.clear(Color::TRANSPARENT);
    if let Some(quad) = hover {
        outline(raster, quad, origin, true);
    }
    for quad in selected {
        outline(raster, quad, origin, false);
    }
    if let Some([l, t, r, b]) = marquee {
        let x = f64::from(origin[0]);
        let y = f64::from(origin[1]);
        let rect = Rect::new(
            (l - x) as f32,
            (t - y) as f32,
            (r - x) as f32,
            (b - y) as f32,
        );
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from_argb(38, 108, 174, 247));
        raster.draw_rect(rect, &paint);
        paint.set_style(paint::Style::Stroke);
        paint.set_stroke_width(1.5);
        paint.set_color(Color::from_rgb(170, 210, 245));
        raster.draw_rect(rect, &paint);
    }

    let row_bytes = size[0] as usize * 4;
    let mut rgba = vec![0; row_bytes * size[1] as usize];
    let straight = ImageInfo::new(dimensions, ColorType::RGBA8888, AlphaType::Unpremul, None);
    ensure!(
        surface.read_pixels(&straight, &mut rgba, row_bytes, (0, 0)),
        "read editor HUD"
    );
    Ok(Some(HudImage {
        origin,
        size,
        rgba: Arc::new(rgba),
    }))
}

fn outline(raster: &Canvas, quad: Quad, origin: [u32; 2], hover: bool) {
    let mut path = PathBuilder::new();
    for (index, [x, y]) in quad.into_iter().enumerate() {
        let point = (
            (x - f64::from(origin[0])) as f32,
            (y - f64::from(origin[1])) as f32,
        );
        if index == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path.close();
    let path = path.detach();
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(paint::Style::Stroke);
    paint.set_stroke_join(paint::Join::Round);
    paint.set_stroke_width(if hover { 3. } else { 4. });
    paint.set_color(Color::from_argb(if hover { 122 } else { 192 }, 14, 26, 43));
    raster.draw_path(&path, &paint);
    paint.set_stroke_width(if hover { 1. } else { 2. });
    paint.set_color(Color::from_argb(
        if hover { 192 } else { 255 },
        170,
        210,
        245,
    ));
    raster.draw_path(&path, &paint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::LayerRef;

    #[test]
    fn selection_outline_is_tight_and_has_a_transparent_interior() -> Result<()> {
        let mut geometry = LayerGeometry {
            target: LayerRef {
                occurrence: "V2:card@0".into(),
                layers: vec![0],
            },
            visibility: Visibility::Visible,
            local_frame: Some(0),
            local_bounds: None,
            local_to_composition: None,
            composition_quad: Some([[10., 5.], [30., 5.], [30., 20.], [10., 20.]]),
            clips: Vec::new(),
        };
        let image =
            render_selection(std::slice::from_ref(&geometry), None, None, [32, 24])?.unwrap();
        assert_eq!(image.origin, [6, 1]);
        assert_eq!(image.size, [26, 23]);
        let pixel = |x: usize, y: usize| {
            let offset = (y * image.size[0] as usize + x) * 4;
            &image.rgba[offset..offset + 4]
        };
        assert_eq!(pixel(14, 4), [170, 210, 245, 255]);
        assert_eq!(pixel(14, 12), [0, 0, 0, 0]);
        geometry.visibility = Visibility::Disabled;
        assert!(render_selection(&[geometry], None, None, [32, 24])?.is_none());
        Ok(())
    }
}
