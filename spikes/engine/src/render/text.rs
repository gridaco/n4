use std::{fs, ops::Range, path::Path};

use crate::{
    MAX_SURFACE_DIMENSION,
    graphics::{TextLine, TextMetrics, Underline},
    project::{Backplate, Title},
};
use anyhow::{Context, Result, ensure};
use skia_safe::{
    AlphaType, BlendMode, Canvas, Color, ColorType, Font, FontMgr, GlyphId, ImageInfo, Paint,
    Point, Rect, Typeface,
    canvas::SaveLayerRec,
    color_filters, surfaces,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextStyle, TypefaceFontProvider,
    },
};
use unicode_segmentation::UnicodeSegmentation;

pub struct Paragraph {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub lines: usize,
    pub plate_size: Option<[u32; 2]>,
}

/// Shape the complete paragraph at its authored width and paint white text into
/// a transparent premultiplied RGBA8 surface. Placement belongs to composition.
pub fn paint_title(
    text: &str,
    font_path: &Path,
    font_size: u32,
    width: u32,
    plate: Option<Backplate>,
) -> Result<Paragraph> {
    paint_with_face(
        text,
        font_path,
        load_face(font_path)?,
        font_size,
        width,
        plate,
    )
}

pub(crate) fn paint_project_title(title: &Title) -> Result<Option<Paragraph>> {
    if title.text.is_empty() && title.backplate.is_none() {
        return Ok(None);
    }
    paint_title(
        &title.text,
        &title.font,
        title.font_size,
        title.width,
        title.backplate,
    )
    .map(Some)
}

pub(crate) fn load_face(font_path: &Path) -> Result<Typeface> {
    let bytes = fs::read(font_path)
        .with_context(|| format!("read paragraph font {}", font_path.display()))?;
    FontMgr::new()
        .new_from_data(&bytes, 0)
        .with_context(|| format!("decode paragraph font {}", font_path.display()))
}

pub(crate) fn paint_with_face(
    text: &str,
    font_path: &Path,
    typeface: Typeface,
    font_size: u32,
    width: u32,
    plate: Option<Backplate>,
) -> Result<Paragraph> {
    let shaped = layout(text, font_path, typeface, font_size, width)?;
    paint_paragraph(&shaped, width, plate)
}

fn layout(
    text: &str,
    font_path: &Path,
    typeface: Typeface,
    font_size: u32,
    width: u32,
) -> Result<skia_safe::textlayout::Paragraph> {
    ensure!(font_size > 0, "paragraph font size must be positive");
    ensure!(
        (1..=MAX_SURFACE_DIMENSION).contains(&width),
        "paragraph width must be between 1 and {MAX_SURFACE_DIMENSION} pixels"
    );

    // Only the supplied face participates in shaping. A missing glyph must
    // never pick a machine-local font.
    let family = "n4-supplied-face";
    let mut faces = TypefaceFontProvider::new();
    faces.register_typeface(typeface, family);
    let mut fonts = FontCollection::new();
    fonts.set_asset_font_manager(FontMgr::from(faces));
    fonts.disable_font_fallback();

    let mut style = TextStyle::new();
    style
        .set_font_families(&[family])
        .set_font_size(font_size as f32)
        .set_color(Color::WHITE);
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_style(&style);
    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
    builder.add_text(text);
    let mut shaped = builder.build();
    shaped.layout(width as f32);
    ensure!(
        shaped.unresolved_glyphs().unwrap_or(0) == 0,
        "paragraph contains glyphs missing from {}",
        font_path.display()
    );
    Ok(shaped)
}

fn paint_paragraph(
    shaped: &skia_safe::textlayout::Paragraph,
    width: u32,
    plate: Option<Backplate>,
) -> Result<Paragraph> {
    let [px, py] = plate.map_or([0, 0], |b| b.padding.map(u32::from));
    let height = shaped.height().ceil().max(1.0) as u32;
    let (raster_width, raster_height) =
        (width.saturating_add(2 * px), height.saturating_add(2 * py));
    ensure!(
        raster_width <= MAX_SURFACE_DIMENSION && raster_height <= MAX_SURFACE_DIMENSION,
        "padded paragraph exceeds the {MAX_SURFACE_DIMENSION}-pixel surface limit"
    );
    let plate_size = plate.map(|_| {
        [
            (shaped.longest_line().ceil().max(1.) as u32).min(width) + 2 * px,
            raster_height,
        ]
    });
    let info = ImageInfo::new(
        (raster_width as i32, raster_height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let mut rgba = super::transparent_pixels(info.compute_min_byte_size())
        .context("allocate paragraph surface")?;
    {
        let mut surface = surfaces::wrap_pixels(&info, &mut rgba, None, None)
            .context("create paragraph surface")?;
        if let Some((plate, [w, h])) = plate.zip(plate_size) {
            let [r, g, b, a] = plate.rgba;
            let mut brush = Paint::default();
            brush.set_color(Color::from_argb(a, r, g, b));
            surface
                .canvas()
                .draw_rect(Rect::from_xywh(0., 0., w as f32, h as f32), &brush);
        }
        shaped.paint(surface.canvas(), (px as f32, py as f32));
    }
    Ok(Paragraph {
        width: raster_width,
        height: raster_height,
        plate_size,
        rgba,
        lines: shaped.line_number(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphStyle {
    pub rgba: [u8; 4],
    pub offset: [f32; 2],
}

struct GlyphRun {
    font: Font,
    origin: Point,
    glyphs: Vec<GlyphId>,
    positions: Vec<Point>,
    ink: Vec<Rect>,
    source: Vec<Range<usize>>,
    #[cfg(feature = "host-audio")]
    line: usize,
}

impl GlyphRun {
    fn paint_group(
        &self,
        canvas: &Canvas,
        range: Range<usize>,
        style: GlyphStyle,
        min: [f32; 2],
        brush: &Paint,
    ) {
        let [r, g, b, a] = style.rgba;
        if a == 0 {
            return;
        }
        let origin = (
            self.origin.x + style.offset[0] - min[0],
            self.origin.y + style.offset[1] - min[1],
        );
        // Skia tunes glyph mask gamma to the foreground luminance.
        // Color the finished white coverage to preserve the existing
        // label painter's edges as an artist changes fill.
        let tint = style.rgba != [255; 4];
        if tint {
            let mut bounds = Rect::default();
            for ink in &self.ink[range.clone()] {
                bounds.join(ink);
            }
            bounds.offset(origin);
            bounds.outset((1., 1.));
            let mut paint = Paint::default();
            paint.set_color_filter(color_filters::blend(
                Color::from_argb(a, r, g, b),
                BlendMode::SrcIn,
            ));
            canvas.save_layer(&SaveLayerRec::default().bounds(&bounds).paint(&paint));
        }
        canvas.draw_glyphs_at(
            &self.glyphs[range.clone()],
            &self.positions[range.clone()],
            origin,
            &self.font,
            brush,
        );
        if tint {
            canvas.restore();
        }
    }
}

/// A wrapped text layer in paint order. Positions and logical source ranges survive
/// paint, visibility and offset edits for the lifetime of this layout.
pub(crate) struct ShapedLabel {
    pub(crate) metrics: TextMetrics,
    runs: Vec<GlyphRun>,
}

impl ShapedLabel {
    pub(crate) fn new(
        text: &str,
        font_path: &Path,
        face: Typeface,
        size: u32,
        width: u32,
    ) -> Result<Self> {
        let mut paragraph = layout(text, font_path, face, size, width)?;
        let height = paragraph.height().ceil().max(1.) as u32;
        ensure!(
            height <= MAX_SURFACE_DIMENSION,
            "paragraph exceeds height limit"
        );
        let lines: Vec<_> = paragraph
            .get_line_metrics()
            .into_iter()
            .map(|line| TextLine {
                source_utf8: {
                    let bytes = paragraph.get_actual_text_range(line.line_number, false);
                    [bytes.start, bytes.end]
                },
                left: line.left,
                right: line.left + line.width,
                baseline: line.baseline,
                top: line.baseline - line.ascent,
                bottom: line.baseline + line.descent,
            })
            .collect();
        let last_line_right_baseline = lines.last().map(|line| [line.right, line.baseline]);
        let last_line_left_baseline = lines.last().map(|line| [line.left, line.baseline]);
        let metrics = TextMetrics {
            width,
            height,
            lines,
            last_line_right_baseline,
            last_line_left_baseline,
        };
        let mut runs = Vec::new();
        paragraph.visit(|_line, info| {
            if let Some(info) = info {
                let source = info.utf8_starts()[..info.count()]
                    .iter()
                    .map(|&start| start as usize..start as usize)
                    .collect();
                let mut ink = vec![Rect::default(); info.count()];
                info.font().get_bounds(info.glyphs(), &mut ink, None);
                for (bounds, pos) in ink.iter_mut().zip(info.positions()) {
                    bounds.offset(*pos);
                }
                runs.push(GlyphRun {
                    font: info.font().clone(),
                    origin: info.origin(),
                    glyphs: info.glyphs().to_vec(),
                    positions: info.positions().to_vec(),
                    ink,
                    source,
                    #[cfg(feature = "host-audio")]
                    line: _line as usize,
                });
            }
        });

        // The visitor reports starts in visual order. Recover the next logical
        // start even for right-to-left runs and repeated starts on mark glyphs.
        let mut starts: Vec<_> = runs
            .iter()
            .flat_map(|r| r.source.iter().map(|s| s.start))
            .collect();
        starts.push(text.len());
        starts.sort_unstable();
        starts.dedup();
        let graphemes: Vec<_> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect();
        for run in &mut runs {
            for source in &mut run.source {
                let start = source.start;
                ensure!(start < text.len(), "invalid glyph cluster offset {start}");
                let unit = graphemes.partition_point(|&i| i <= start) - 1;
                let next_start = starts[starts.partition_point(|&i| i <= start)];
                // A gap can be a ligature, or trailing whitespace omitted by the
                // line painter. Ask the shaper for the actual cluster extent.
                let end = if next_start > graphemes[unit + 1] {
                    paragraph
                        .get_glyph_cluster_at(start)
                        .map_or(graphemes[unit + 1], |info| info.text_range.end)
                } else {
                    next_start
                };
                ensure!(
                    end > start && end <= text.len(),
                    "invalid glyph cluster extent {start}..{end}"
                );
                let last = graphemes.partition_point(|&i| i < end);
                *source = graphemes[unit]..graphemes[last];
            }
        }
        Ok(Self { metrics, runs })
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        [self.metrics.width, self.metrics.height]
    }

    /// Vertical surface extension for dots beyond the paragraph's typographic box.
    pub(crate) fn decoration_bounds(&self, underline: Option<&Underline>) -> ([i32; 2], [u32; 2]) {
        let [width, height] = self.size();
        let mut top = 0;
        let mut bottom = height as i32;
        if let Some(Underline::Dotted {
            diameter, offset, ..
        }) = underline
        {
            let radius = f64::from(*diameter) / 2.;
            for line in self.metrics.lines.iter().filter(|l| l.right > l.left) {
                let y = line.baseline + f64::from(*offset);
                top = top.min((y - radius).floor() as i32);
                bottom = bottom.max((y + radius).ceil() as i32);
            }
        }
        ([0, top], [width, (bottom - top) as u32])
    }

    #[cfg(test)]
    pub(crate) fn paint(
        &self,
        style_for: impl Fn(Range<usize>) -> GlyphStyle,
    ) -> Result<crate::graphics::Raster> {
        self.paint_decorated(style_for, None)
    }

    pub(crate) fn paint_decorated(
        &self,
        style_for: impl Fn(Range<usize>) -> GlyphStyle,
        underline: Option<&Underline>,
    ) -> Result<crate::graphics::Raster> {
        if let Some(u) = underline {
            u.validate()?;
        }
        let (origin, extent, styles) = self.measure_styles(style_for, underline)?;
        let min = origin.map(|v| v as f32);
        let info = ImageInfo::new(
            (extent[0] as i32, extent[1] as i32),
            ColorType::RGBA8888,
            AlphaType::Premul,
            None,
        );
        let mut rgba = super::transparent_pixels(info.compute_min_byte_size())
            .context("allocate shaped label")?;
        {
            let mut surface = surfaces::wrap_pixels(&info, &mut rgba, None, None)
                .context("create shaped label surface")?;
            let canvas = surface.canvas();
            let mut brush = Paint::default();
            if let Some(Underline::Dotted {
                rgba: [r, g, b, a],
                diameter,
                spacing,
                offset,
            }) = underline
            {
                let radius = f32::from(*diameter) / 2.;
                brush
                    .set_anti_alias(true)
                    .set_color(Color::from_argb(*a, *r, *g, *b));
                for line in &self.metrics.lines {
                    let left = (line.left as f32).max(0.);
                    let right = (line.right as f32).min(self.metrics.width as f32);
                    let y = line.baseline as f32 + f32::from(*offset) - min[1];
                    let mut x = left + radius;
                    while x + radius <= right {
                        canvas.draw_circle((x - min[0], y), radius, &brush);
                        x += f32::from(*spacing);
                    }
                }
                brush = Paint::default();
            }
            brush.set_color(Color::WHITE);
            let mut remaining = styles.as_slice();
            for run in &self.runs {
                let (styled, rest) = remaining.split_at(run.glyphs.len());
                remaining = rest;
                let mut start = 0;
                for group in styled.chunk_by(|a, b| a == b) {
                    let style = group[0];
                    let end = start + group.len();
                    run.paint_group(canvas, start..end, style, min, &brush);
                    start = end;
                }
            }
        }
        Ok(crate::graphics::Raster {
            origin,
            size: extent,
            rgba,
        })
    }

    fn measure_styles(
        &self,
        style_for: impl Fn(Range<usize>) -> GlyphStyle,
        underline: Option<&Underline>,
    ) -> Result<([i32; 2], [u32; 2], Vec<GlyphStyle>)> {
        let (at, decorated) = self.decoration_bounds(underline);
        let mut min = at.map(|v| v as f32);
        let mut max = [
            0.,
            (at[1] as f32 + decorated[1] as f32) - self.metrics.height as f32,
        ];
        let mut styles = Vec::new();
        for run in &self.runs {
            for source in &run.source {
                let style = style_for(source.clone());
                if style.rgba[3] > 0 {
                    for axis in 0..2 {
                        ensure!(style.offset[axis].is_finite(), "text offset must be finite");
                        min[axis] = min[axis].min(style.offset[axis].floor());
                        max[axis] = max[axis].max(style.offset[axis].ceil());
                    }
                }
                styles.push(style);
            }
        }
        let mut extent = self.size();
        for axis in 0..2 {
            let expanded = extent[axis] as f32 + (max[axis] - min[axis]);
            ensure!(
                expanded <= MAX_SURFACE_DIMENSION as f32,
                "offset text exceeds surface limit"
            );
            extent[axis] = expanded as u32;
        }
        Ok((min.map(|v| v as i32), extent, styles))
    }

    #[cfg(feature = "host-audio")]
    pub(crate) fn selection_geometry(
        &self,
        underline: Option<&Underline>,
        style_for: Option<impl Fn(Range<usize>) -> GlyphStyle>,
    ) -> Result<crate::graphics::TextGeometry> {
        let mut regions = Vec::new();
        let raster_bounds = if let Some(style_for) = style_for {
            let (origin, extent, styles) = self.measure_styles(style_for, underline)?;
            let mut styled = styles.into_iter();
            for run in &self.runs {
                let line = &self.metrics.lines[run.line];
                let mut advances = vec![0.; run.glyphs.len()];
                run.font.get_widths(&run.glyphs, &mut advances);
                for (i, (advance, style)) in advances.iter().zip(&mut styled).enumerate() {
                    if style.rgba[3] == 0 || run.source[i].start >= line.source_utf8[1] {
                        continue;
                    }
                    let x = run.positions[i].x;
                    let ink = run.ink[i];
                    let right = run
                        .positions
                        .get(i + 1)
                        .filter(|next| next.x > x)
                        .map_or(x + advance, |next| next.x);
                    let left = if ink.is_empty() { x } else { x.min(ink.left) };
                    let right = if ink.is_empty() {
                        right
                    } else {
                        right.max(ink.right)
                    };
                    let offset = f64::from(run.origin.x + style.offset[0]);
                    regions.push([
                        f64::from(left) + offset,
                        line.top + f64::from(style.offset[1]),
                        f64::from(right) + offset,
                        line.bottom + f64::from(style.offset[1]),
                    ]);
                }
            }
            (origin, extent)
        } else {
            regions.extend(
                self.metrics
                    .lines
                    .iter()
                    .map(|l| [l.left, l.top, l.right, l.bottom]),
            );
            self.decoration_bounds(underline)
        };
        if let Some(Underline::Dotted {
            rgba,
            diameter,
            spacing,
            offset,
        }) = underline
            && rgba[3] > 0
        {
            let d = f64::from(*diameter);
            for line in &self.metrics.lines {
                let left = line.left.max(0.);
                let right = line.right.min(f64::from(self.metrics.width));
                if right - left >= d {
                    let intervals = ((right - left - d) / f64::from(*spacing)).floor();
                    let y = line.baseline + f64::from(*offset);
                    regions.push([
                        left,
                        y - d / 2.,
                        left + intervals * f64::from(*spacing) + d,
                        y + d / 2.,
                    ]);
                }
            }
        }
        let (origin, size) = raster_bounds;
        let [x, y] = origin.map(f64::from);
        for region in &mut regions {
            let [left, top, right, bottom] = *region;
            *region = [
                left.max(x),
                top.max(y),
                right.min(x + f64::from(size[0])),
                bottom.min(y + f64::from(size[1])),
            ];
        }
        regions.retain(|[left, top, right, bottom]| left < right && top < bottom);
        Ok(crate::graphics::TextGeometry {
            regions,
            raster_bounds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const WHITE: GlyphStyle = GlyphStyle {
        rgba: [255; 4],
        offset: [0.; 2],
    };
    const HIDDEN: GlyphStyle = GlyphStyle {
        rgba: [0; 4],
        offset: [0.; 2],
    };

    fn shaping_font() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf")
    }

    #[test]
    fn neutral_runs_match_the_paragraph_painter_at_multiple_widths() -> Result<()> {
        let font = shaping_font();
        let text = "A ffi x e\u{301} אבג";
        for width in [600, 85] {
            let expected = paint_title(text, &font, 32, width, None)?;
            let layout = ShapedLabel::new(text, &font, load_face(&font)?, 32, width)?;
            let raster = layout.paint(|_| WHITE)?;
            assert_eq!(layout.size(), [expected.width, expected.height]);
            assert_eq!(raster.origin, [0, 0]);
            assert_eq!(raster.size, layout.size());
            let differences = raster
                .rgba
                .iter()
                .zip(&expected.rgba)
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(differences, 0, "neutral glyph painting at width {width}");
            let color = [241, 107, 27, 255];
            let tinted = layout.paint(|_| GlyphStyle {
                rgba: color,
                ..WHITE
            })?;
            for (pixel, white) in tinted
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.rgba.as_chunks::<4>().0)
            {
                assert_eq!(pixel[3], white[3], "fill must preserve glyph coverage");
                for i in 0..3 {
                    assert_eq!(
                        pixel[i],
                        ((u16::from(color[i]) * u16::from(white[3]) + 127) / 255) as u8
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn ligatures_graphemes_and_bidirectional_runs_use_logical_source_ranges() -> Result<()> {
        let font = shaping_font();
        let text = "ffi e\u{301} אבג";
        let layout = ShapedLabel::new(text, &font, load_face(&font)?, 34, 300)?;
        let ranges: Vec<_> = layout
            .runs
            .iter()
            .flat_map(|r| &r.source)
            .cloned()
            .collect();
        assert!(
            ranges.contains(&(0..3)),
            "fixture must actually shape the ffi ligature: {ranges:?}"
        );
        assert!(!ranges.contains(&(0..1)));
        assert!(
            ranges.contains(&(4..7)),
            "base and combining accent must share a range"
        );
        let visible =
            |end: usize| layout.paint(|range| if range.end <= end { WHITE } else { HIDDEN });
        assert!(visible(2)?.rgba.iter().all(|&v| v == 0));
        assert!(visible(3)?.rgba.iter().any(|&v| v != 0));
        assert_eq!(visible(4)?.rgba, visible(5)?.rgba);
        let left_edge = |start: usize| -> Result<usize> {
            let raster = layout.paint(|range| if range.start == start { WHITE } else { HIDDEN })?;
            raster
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| p[3] != 0)
                .map(|(i, _)| i % raster.size[0] as usize)
                .min()
                .context("expected visible Hebrew glyph")
        };
        assert!(left_edge(8)? > left_edge(10)? && left_edge(10)? > left_edge(12)?);
        let multiline = ShapedLabel::new("ffi  \nA", &font, load_face(&font)?, 34, 300)?;
        let source: Vec<_> = multiline.runs.iter().flat_map(|r| &r.source).collect();
        assert!(
            source.contains(&&(0..3)),
            "a ligature followed by a break must reveal on i"
        );
        assert!(source.contains(&&(6..7)));
        Ok(())
    }

    #[test]
    fn selected_word_moves_without_reflow_or_clipping() -> Result<()> {
        let font = shaping_font();
        let layout = ShapedLabel::new("A ffi A", &font, load_face(&font)?, 31, 180)?;
        let neutral = layout.paint(|_| WHITE)?;
        let style = |range| {
            if range == (2..5) {
                GlyphStyle {
                    rgba: [255, 0, 0, 255],
                    offset: [0., -12.],
                }
            } else {
                WHITE
            }
        };
        let moved = layout.paint(style)?;
        assert_eq!(moved.origin, [0, -12]);
        let width = neutral.size[0] as usize;
        let neighbors = layout.paint(|range| if range == (2..5) { HIDDEN } else { WHITE })?;
        let ligature = layout.paint(|range| if range == (2..5) { WHITE } else { HIDDEN })?;
        let mut red = 0;
        for (idx, pixel) in moved.rgba.as_chunks::<4>().0.iter().enumerate() {
            let neighbor = idx
                .checked_sub(width * 12)
                .and_then(|i| neighbors.rgba.get(i * 4 + 3))
                .copied()
                .unwrap_or(0);
            let letter = ligature.rgba.get(idx * 4 + 3).copied().unwrap_or(0);
            assert_eq!(
                pixel,
                &[
                    neighbor.max(letter),
                    neighbor,
                    neighbor,
                    neighbor.max(letter)
                ]
            );
            red += usize::from(letter != 0);
        }
        assert!(red > 0);
        #[cfg(feature = "host-audio")]
        {
            let selected = layout.selection_geometry(
                None,
                Some(|range| {
                    if range == (2..5) {
                        style(range)
                    } else {
                        HIDDEN
                    }
                }),
            )?;
            assert_eq!(selected.raster_bounds, (moved.origin, moved.size));
            assert!(!selected.regions.is_empty());
            for [left, top, right, bottom] in selected.regions {
                let original = &layout.metrics.lines[0];
                assert!(left > original.left && right < original.right);
                assert_eq!(top, (original.top - 12.).max(f64::from(moved.origin[1])));
                assert_eq!(bottom, original.bottom - 12.);
            }
        }
        Ok(())
    }

    #[test]
    fn supplied_font_wraps_and_paints_transparent_white() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf");
        let sentence = "A frame worth keeping";
        let wide = paint_title(sentence, &path, 32, 450, None)?;
        let narrow = paint_title(sentence, &path, 32, 240, None)?;
        assert_eq!(wide.lines, 1);
        assert_eq!(narrow.lines, 2);
        assert!(narrow.height > wide.height);
        let mut transparent = false;
        let mut ink = false;
        for pixel in narrow.rgba.as_chunks::<4>().0 {
            assert_eq!(pixel, &[pixel[3]; 4], "white premultiplied glyph coverage");
            transparent |= pixel[3] == 0;
            ink |= pixel[3] > 0;
        }
        assert!(transparent && ink);
        Ok(())
    }
}
