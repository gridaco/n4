//! Native timeline geometry is expressed in window points. Only painting converts it to
//! drawable pixels, so high-DPI input and the playhead use exactly the same coordinate space.
use anyhow::{Context, Result, ensure};
use n4_engine::{FrameRate, Project, preview::Thumbnail};
use sdl2::{
    pixels::{Color as SdlColor, PixelFormatEnum},
    rect::Rect as SdlRect,
    render::{ScaleMode, Texture, TextureCreator, WindowCanvas},
    video::WindowContext,
};
use skia_safe::{
    AlphaType, Canvas, Color, ColorType, Font, FontMgr, ImageInfo, Paint, Rect, Typeface, surfaces,
};
use std::{collections::BTreeMap, sync::Arc};

const PANEL: Color = Color::from_rgb(32, 34, 38);
const EDGE: Color = Color::from_rgb(61, 64, 70);
const TEXT: Color = Color::from_rgb(233, 236, 238);
const MUTED: Color = Color::from_rgb(163, 169, 177);
const GUTTER: i32 = 79;
pub(super) const STRIP_HEIGHT: u32 = 164;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Toggle,
    Inspector,
    Seek(u32),
    None,
}

#[derive(Clone, Copy)]
pub struct Layout {
    w: i32,
    h: i32,
    panel: i32,
    left: i32,
    right: i32,
    total: u32,
}

impl Layout {
    pub fn new((w, h): (u32, u32), total_frames: u32) -> Self {
        let w = w.clamp(1, i32::MAX as u32) as i32;
        let h = h.clamp(1, i32::MAX as u32) as i32;
        let left = GUTTER.min(w / 4);
        Self {
            w,
            h,
            panel: (h - STRIP_HEIGHT as i32).max(0),
            left,
            right: (w - 16).max(left + 1).min(w),
            total: total_frames.max(1),
        }
    }

    pub fn frame_at(&self, x: i32) -> u32 {
        let span = (self.right - self.left - 1).max(1);
        let offset = (x - self.left).clamp(0, span);
        ((u64::from(self.total - 1) * offset as u64 + span as u64 / 2) / span as u64) as u32
    }

    fn x_at(&self, frame: u32) -> i32 {
        let span = (self.right - self.left - 1).max(0);
        let last = self.total.saturating_sub(1).max(1);
        self.left
            + ((u64::from(frame.min(last)) * span as u64 + u64::from(last) / 2) / u64::from(last))
                as i32
    }

    pub fn point_at(&self, frame: u32) -> (i32, i32) {
        (self.x_at(frame), self.panel + 92)
    }

    pub fn hit(&self, x: i32, y: i32) -> Hit {
        let y = y - self.panel;
        if (12..57).contains(&x) && (6..44).contains(&y) {
            Hit::Toggle
        } else if (self.w - 108..self.w - 12).contains(&x) && (6..44).contains(&y) {
            Hit::Inspector
        } else if (self.left..self.right).contains(&x) && (50..155).contains(&y) {
            Hit::Seek(self.frame_at(x))
        } else {
            Hit::None
        }
    }

    fn boundary(&self, frame: u32) -> f32 {
        self.left as f32
            + (self.right - self.left) as f32 * (frame as f64 / self.total as f64) as f32
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Scale(f32, f32);

impl Scale {
    fn rect(self, x: f32, y: f32, w: f32, h: f32) -> SdlRect {
        let left = (x * self.0).round() as i32;
        let top = (y * self.1).round() as i32;
        let right = ((x + w) * self.0).round() as i32;
        let bottom = ((y + h) * self.1).round() as i32;
        SdlRect::new(
            left,
            top,
            (right - left).max(1) as u32,
            (bottom - top).max(1) as u32,
        )
    }
}

fn brush(color: Color) -> Paint {
    let mut paint = Paint::default();
    paint.set_color(color).set_anti_alias(true);
    paint
}

fn fill(canvas: &Canvas, x: f32, y: f32, w: f32, h: f32, color: Color) {
    canvas.draw_rect(Rect::from_xywh(x, y, w, h), &brush(color));
}

fn label(canvas: &Canvas, font: &Font, text: &str, x: f32, baseline: f32, color: Color) {
    canvas.draw_str(text, (x, baseline), font, &brush(color));
}

fn timestamp(frame: u32, fps: FrameRate) -> String {
    let millis = u128::from(frame) * u128::from(fps.1) * 1000 / u128::from(fps.0);
    format!(
        "{:02}:{:02}.{:03}",
        millis / 60_000,
        millis / 1000 % 60,
        millis % 1000
    )
}

fn time_width(text: &str) -> f32 {
    text.bytes()
        .map(|b| if b.is_ascii_digit() { 10. } else { 4. })
        .sum()
}

struct ShotBand {
    start: u32,
    end: u32,
    name: String,
    label_width: f32,
    cameras: Vec<n4_engine::camera::CameraCut>,
}

fn shot_name(shot: &n4_engine::Shot, index: usize) -> String {
    if let Some(name) = &shot.name {
        return name.clone();
    }
    shot.source
        .as_ref()
        .and_then(|path| path.file_name())
        .map_or_else(
            || format!("Shot {}", index + 1),
            |name| name.to_string_lossy().into_owned(),
        )
}

pub struct Timeline<'a> {
    creator: &'a TextureCreator<WindowContext>,
    fps: FrameRate,
    total: u32,
    size: [u32; 2],
    shots: Vec<ShotBand>,
    has_audio: bool,
    waveform: Arc<[f32]>,
    thumbnails: BTreeMap<u32, Texture<'a>>,
    face: Typeface,
    strip: Option<Texture<'a>>,
    // Digits and the two transport icons are painted once per display scale.
    controls: Option<Texture<'a>>,
    last_geometry: Option<(i32, i32, Scale)>,
}

impl<'a> Timeline<'a> {
    pub fn new(creator: &'a TextureCreator<WindowContext>, project: &Project) -> Result<Self> {
        let total = project.duration()?;
        ensure!(total > 0, "timeline requires at least one frame");
        let face = FontMgr::new()
            .new_from_data(
                skia_safe::Data::new_copy(include_bytes!("../../../fixtures/fonts/N4GridSans.ttf")),
                None,
            )
            .context("load timeline UI font")?;
        let small = Font::new(face.clone(), 10.5);
        let mut start = 0;
        let mut shots = Vec::new();
        for (index, shot) in project.shots.iter().enumerate() {
            let end = start + shot.duration;
            let name = shot_name(shot, index);
            let label_width = small.measure_str(&name, None).0;
            let cameras = if shot.camera_cuts.is_empty() {
                shot.camera
                    .iter()
                    .map(|id| n4_engine::camera::CameraCut {
                        start: 0,
                        end: shot.duration,
                        camera: id.clone(),
                    })
                    .collect()
            } else {
                shot.camera_cuts.clone()
            };
            shots.push(ShotBand {
                start,
                end,
                name,
                label_width,
                cameras,
            });
            start = end;
        }
        Ok(Self {
            creator,
            fps: project.fps,
            total,
            size: project.size,
            shots,
            face,
            has_audio: !project.audio.is_empty(),
            waveform: Arc::from([]),
            thumbnails: BTreeMap::new(),
            strip: None,
            controls: None,
            last_geometry: None,
        })
    }

    pub fn set_thumbnail(&mut self, frame: u32, thumb: &Thumbnail) -> Result<()> {
        let [width, height] = thumb.size;
        let mut texture =
            self.creator
                .create_texture_static(PixelFormatEnum::RGBA32, width, height)?;
        texture.update(None, &thumb.rgba, width as usize * 4)?;
        texture.set_scale_mode(ScaleMode::Linear);
        self.thumbnails.insert(frame, texture);
        Ok(())
    }

    pub fn set_waveform(&mut self, peaks: Arc<[f32]>) {
        self.waveform = peaks;
        self.strip = None;
    }

    fn paint_strip(&self, canvas: &Canvas, layout: Layout) {
        canvas.clear(PANEL);
        let w = layout.w as f32;
        let left = layout.left as f32;
        let right = layout.right as f32;
        let ui = Font::new(self.face.clone(), 12.0);
        let small = Font::new(self.face.clone(), 10.5);
        fill(canvas, 0., 0., w, 1., EDGE);
        canvas.draw_round_rect(
            Rect::from_xywh(14., 9., 39., 31.),
            6.,
            6.,
            &brush(Color::from_rgb(59, 64, 70)),
        );
        let total = timestamp(self.total, self.fps);
        let total_x = 66. + time_width(&total) + 10.;
        label(canvas, &ui, &format!("/  {total}"), total_x, 29., MUTED);
        if w > 510. {
            label(canvas, &small, "FRAME", total_x + 103., 28., MUTED);
        }
        let hints = "Space  Play / pause     ← →  Step";
        let hint_width = ui.measure_str(hints, None).0;
        if w - hint_width - 120. > total_x + 250. {
            label(canvas, &ui, hints, w - hint_width - 126., 29., MUTED);
        }
        canvas.draw_round_rect(
            Rect::from_xywh(w - 108., 9., 96., 31.),
            5.,
            5.,
            &brush(Color::from_rgb(56, 60, 66)),
        );
        label(canvas, &ui, "Inspector   I", w - 94., 29., TEXT);
        fill(canvas, 0., 49., w, 1., EDGE);

        let span = (right - left).max(1.);
        let seconds = f64::from(self.total) * f64::from(self.fps.1) / f64::from(self.fps.0);
        let wanted = seconds * 90. / span as f64;
        let decade = 10_f64.powf(wanted.max(0.001).log10().floor());
        let step = [1., 2., 5., 10.]
            .into_iter()
            .map(|n| n * decade)
            .find(|v| *v >= wanted)
            .unwrap_or(10. * decade);
        let tick_count = (seconds / step).floor() as u32;
        for i in 0..=tick_count {
            let t = f64::from(i) * step;
            let x = left + span * (t / seconds) as f32;
            fill(canvas, x, 68., 1., 4., EDGE);
            let caption = if step < 1. {
                format!("{t:.1}s")
            } else {
                format!("{}:{:02}", t as u64 / 60, t as u64 % 60)
            };
            if x + small.measure_str(&caption, None).0 < right {
                label(canvas, &small, &caption, x + 2., 64., MUTED);
            }
        }
        label(canvas, &ui, "Picture", 14., 100., TEXT);
        label(canvas, &ui, "Mix", 14., 139., TEXT);
        fill(canvas, left, 74., span, 44., Color::from_rgb(54, 58, 73));

        for (index, shot) in self.shots.iter().enumerate() {
            let a = layout.boundary(shot.start);
            let b = layout.boundary(shot.end);
            if b <= a {
                continue;
            }
            canvas.save();
            canvas.clip_rect(Rect::from_xywh(a, 74., b - a, 44.), None, false);
            fill(
                canvas,
                a,
                74.,
                b - a,
                44.,
                if index % 2 == 0 {
                    Color::from_rgb(52, 58, 74)
                } else {
                    Color::from_rgb(59, 64, 80)
                },
            );

            fill(canvas, a, 74., b - a, 2., Color::from_rgb(148, 159, 208));
            for cut in &shot.cameras {
                let x = layout.boundary(shot.start + cut.start);
                let width = layout.boundary(shot.start + cut.end) - x;
                fill(
                    canvas,
                    x,
                    76.,
                    width.max(1.),
                    14.,
                    Color::from_rgb(65, 59, 46),
                );
                fill(canvas, x, 76., 1., 14., Color::from_rgb(213, 182, 120));
                let cue = format!("CAM  {}", cut.camera);
                if width >= small.measure_str(&cue, None).0 + 8. {
                    label(canvas, &small, &cue, x + 4., 87., TEXT);
                }
            }
            if b - a > 65. {
                let name_w = shot.label_width.min(b - a - 12.);
                canvas.draw_round_rect(
                    Rect::from_xywh(a + 4., 99., name_w + 8., 16.),
                    3.,
                    3.,
                    &brush(Color::from_argb(220, 26, 28, 34)),
                );
                canvas.save();
                canvas.clip_rect(Rect::from_xywh(a + 8., 99., name_w, 16.), None, false);
                label(canvas, &small, &shot.name, a + 8., 111., TEXT);
                canvas.restore();
            }
            if index > 0 {
                fill(canvas, a, 74., 2., 44., PANEL)
            }
            canvas.restore();
        }

        fill(canvas, left, 122., span, 29., Color::from_rgb(34, 61, 58));
        fill(canvas, left, 136., span, 1., Color::from_rgb(66, 102, 94));
        if !self.waveform.is_empty() {
            let max = self
                .waveform
                .iter()
                .copied()
                .fold(0.0_f32, f32::max)
                .max(0.001);
            let bins = self.waveform.len();
            let width = (right - left).max(1.) as usize;
            let ink = brush(Color::from_rgb(110, 189, 165));
            for column in 0..width {
                let start = column * bins / width;
                let end = ((column + 1) * bins / width).max(start + 1).min(bins);
                let peak = self.waveform[start..end]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max);
                let height = (peak / max).clamp(0., 1.) * 11.;
                let x = left + column as f32;
                canvas.draw_line((x, 136. - height), (x, 137. + height), &ink);
            }
        } else if !self.has_audio {
            label(canvas, &small, "No audio", left + 8., 141., MUTED);
        }
    }

    // The ruler, waveform and captions stay resident when background thumbnail jobs finish.
    // Each incoming thumbnail uploads only its own small texture.
    fn draw_filmstrip(
        &self,
        canvas: &mut WindowCanvas,
        layout: Layout,
        scale: Scale,
        strip: &Texture<'_>,
    ) -> Result<()> {
        for shot in &self.shots {
            let a = layout.boundary(shot.start);
            let b = layout.boundary(shot.end);
            let mut tiles = self.thumbnails.range(shot.start..shot.end).peekable();
            if tiles.peek().is_none() {
                continue;
            }
            let mut previous_edge = a + if shot.start == 0 { 0. } else { 2. };
            while let Some((frame, tile)) = tiles.next() {
                let center = layout.boundary(*frame);
                let next_edge = tiles
                    .peek()
                    .map_or(b, |(next, _)| (center + layout.boundary(**next)) * 0.5);
                // Leave distant, unsampled time empty as the filmstrip fills.
                let left = previous_edge.max(center - 76.);
                let right = next_edge.min(center + 76.);
                previous_edge = next_edge;
                if right - left < 1. {
                    continue;
                }
                let top = if shot.cameras.is_empty() { 76. } else { 90. };
                let dst = scale.rect(left, layout.panel as f32 + top, right - left, 118. - top);
                let source = tile.query();
                let fit = (dst.width() as f32 / source.width as f32)
                    .max(dst.height() as f32 / source.height as f32);
                let sw = (dst.width() as f32 / fit)
                    .round()
                    .clamp(1., source.width as f32) as u32;
                let sh = (dst.height() as f32 / fit)
                    .round()
                    .clamp(1., source.height as f32) as u32;
                let crop = SdlRect::new(
                    ((source.width - sw) / 2) as i32,
                    ((source.height - sh) / 2) as i32,
                    sw,
                    sh,
                );
                canvas.copy(tile, crop, dst).map_err(anyhow::Error::msg)?;
            }
            if b - a > 65. {
                let width = shot.label_width.min(b - a - 12.) + 8.;
                let source = scale.rect(a + 4., 99., width, 16.);
                let dst = scale.rect(a + 4., layout.panel as f32 + 99., width, 16.);
                canvas
                    .copy(strip, source, dst)
                    .map_err(anyhow::Error::msg)?;
            }
        }
        Ok(())
    }

    pub fn draw(
        &mut self,
        canvas: &mut WindowCanvas,
        movie: Option<&Texture<'_>>,
        playhead: u32,
        displayed: Option<u32>,
        playing: bool,
        inspector_height: u32,
    ) -> Result<()> {
        let (w, h) = canvas.window().size();
        let (dw, dh) = canvas.output_size().map_err(anyhow::Error::msg)?;
        if w == 0 || h == 0 || dw == 0 || dh == 0 {
            return Ok(());
        }
        let layout = Layout::new((w, h), self.total);
        let scale = Scale(dw as f32 / w as f32, dh as f32 / h as f32);
        let geometry = (layout.w, layout.h, scale);
        if self.last_geometry != Some(geometry) {
            self.strip = None;
            if self.last_geometry.is_none_or(|(_, _, old)| old != scale) {
                self.controls = None;
            }
            self.last_geometry = Some(geometry);
        }
        let panel_y = layout.panel as f32;
        let panel_h = (layout.h - layout.panel) as f32;
        let panel = scale.rect(0., panel_y, w as f32, panel_h);
        if self.strip.is_none() {
            self.strip = Some(raster(
                self.creator,
                scale,
                panel.width(),
                panel.height(),
                |c| self.paint_strip(c, layout),
            )?);
        }
        let sheet = scale.rect(0., 0., 150., 52.);
        if self.controls.is_none() {
            self.controls = Some(raster(
                self.creator,
                scale,
                sheet.width(),
                sheet.height(),
                |c| {
                    c.clear(PANEL);
                    let font = Font::new(self.face.clone(), 15.);
                    for (i, ch) in "0123456789:.".chars().enumerate() {
                        label(c, &font, &ch.to_string(), i as f32 * 12., 17., TEXT);
                    }
                    // Icon rectangles use exactly the same gray as the button underneath.
                    fill(c, 0., 25., 40., 23., Color::from_rgb(59, 64, 70));
                    let mut path = skia_safe::PathBuilder::new();
                    path.move_to((4., 28.))
                        .line_to((4., 43.))
                        .line_to((16., 35.5))
                        .close();
                    c.draw_path(&path.detach(), &brush(TEXT));
                    fill(c, 24., 28., 4., 15., TEXT);
                    fill(c, 31., 28., 4., 15., TEXT);
                },
            )?);
        }
        canvas.set_draw_color(SdlColor::RGB(15, 16, 18));
        canvas.clear();
        if let Some(movie) = movie {
            let available_w = (layout.w - 24).max(1) as f32;
            let movie_bottom = (panel_y - inspector_height as f32).max(1.);
            let available_h = (movie_bottom - 20.).max(1.);
            let fit = (available_w / self.size[0] as f32).min(available_h / self.size[1] as f32);
            let (mw, mh) = (self.size[0] as f32 * fit, self.size[1] as f32 * fit);
            canvas
                .copy(
                    movie,
                    None,
                    scale.rect((w as f32 - mw) / 2., (movie_bottom - mh) / 2., mw, mh),
                )
                .map_err(anyhow::Error::msg)?;
        }
        let copy = |r: std::result::Result<(), String>| r.map_err(anyhow::Error::msg);
        let strip = self.strip.as_ref().unwrap();
        copy(canvas.copy(strip, None, panel))?;
        self.draw_filmstrip(canvas, layout, scale, strip)?;
        let controls = self.controls.as_ref().unwrap();
        let icon_x = if playing { 20. } else { 0. };
        copy(canvas.copy(
            controls,
            scale.rect(icon_x, 25., 20., 22.),
            scale.rect(24., panel_y + 14., 20., 22.),
        ))?;
        let time = timestamp(playhead, self.fps);
        let digits = |renderer: &mut WindowCanvas, text: &str, x: f32, step: f32| -> Result<()> {
            let mut left = x;
            for b in text.bytes() {
                let g = match b {
                    b'0'..=b'9' => b - b'0',
                    b':' => 10,
                    b'.' => 11,
                    _ => continue,
                };
                let advance = if b.is_ascii_digit() { step } else { 4. };
                copy(renderer.copy(
                    controls,
                    scale.rect(f32::from(g) * 12., 0., advance, 22.),
                    scale.rect(left, panel_y + 12., advance, 22.),
                ))?;
                left += advance;
            }
            Ok(())
        };
        digits(canvas, &time, 66., 10.)?;
        if w > 510 {
            let frame_x = 66. + time_width(&timestamp(self.total, self.fps)) + 158.;
            digits(canvas, &playhead.to_string(), frame_x, 9.)?;
        }
        // A subdued cursor shows the actually displayed picture while a seek is still in flight.
        if let Some(frame) = displayed.filter(|f| *f != playhead) {
            canvas.set_draw_color(SdlColor::RGB(165, 169, 174));
            copy(canvas.fill_rect(scale.rect(layout.x_at(frame) as f32, panel_y + 74., 1., 77.)))?;
        }
        let x = layout.x_at(playhead) as f32;
        canvas.set_draw_color(SdlColor::RGB(250, 192, 93));
        copy(canvas.fill_rect(scale.rect(x, panel_y + 67., 1.5, 85.)))?;
        for row in 0..5 {
            let half = (4 - row) as f32;
            copy(canvas.fill_rect(scale.rect(
                x - half,
                panel_y + 64. + row as f32,
                half * 2. + 1.,
                1.,
            )))?;
        }
        Ok(())
    }
}

fn raster<'a>(
    creator: &'a TextureCreator<WindowContext>,
    scale: Scale,
    width: u32,
    height: u32,
    paint: impl FnOnce(&Canvas),
) -> Result<Texture<'a>> {
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let mut bytes = vec![0; info.compute_min_byte_size()];
    {
        let mut surface = surfaces::wrap_pixels(&info, &mut bytes, None, None)
            .context("paint native timeline")?;
        let c = surface.canvas();
        c.scale((scale.0, scale.1));
        paint(c);
    }
    let mut texture = creator.create_texture_static(PixelFormatEnum::RGBA32, width, height)?;
    texture.update(None, &bytes, width as usize * 4)?;
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_last_and_cut_share_the_cursor_transform() {
        let l = Layout::new((1080, 720), 300);
        for f in [0, 1, 149, 150, 299] {
            let (x, y) = l.point_at(f);
            assert_eq!(l.hit(x, y), Hit::Seek(f));
        }
        assert_eq!(l.frame_at(l.left - 200), 0);
        assert_eq!(l.frame_at(l.right + 200), 299);
        assert_eq!(l.hit(35, l.panel + 25), Hit::Toggle);
        assert_eq!(l.hit(80, l.panel - 1), Hit::None);
    }
    #[test]
    fn scales_and_single_frame_projects_keep_endpoints_in_bounds() {
        let l = Layout::new((475, 300), 1);
        assert_eq!(l.frame_at(l.right), 0);
        assert_eq!(l.hit(l.left, l.panel + 93), Hit::Seek(0));
        let r = Scale(2., 2.).rect(14., 9., 39., 31.);
        assert_eq!((r.x(), r.y(), r.width(), r.height()), (28, 18, 78, 62));
    }

    #[test]
    fn strip_labels_follow_the_edit() {
        use serde_json::json;
        for (shot, label) in [
            (
                json!({"duration":1,"background":{"color":[5,5,5]},"name":"Wing underside"}),
                "Wing underside",
            ),
            (
                json!({"duration":1,"source":"media/camera.mov","name":"Interview"}),
                "Interview",
            ),
            (
                json!({"duration":1,"source":"media/camera.mov"}),
                "camera.mov",
            ),
            (
                json!({"duration":1,"background":{"color":[5,5,5]}}),
                "Shot 3",
            ),
        ] {
            assert_eq!(shot_name(&serde_json::from_value(shot).unwrap(), 2), label);
        }
    }
}
