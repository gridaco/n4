//! Paragraph-local measurements shared by decoration, attachment and authoring tools.
use super::{
    Content, Graphic, PaintCache, PaintStats, TextLayoutKey, text_animation::PreparedText,
};
use crate::{MAX_SURFACE_DIMENSION, Project, render::text};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use skia_safe::Typeface;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Weak},
};

#[cfg(feature = "host-audio")]
use {
    super::{TextAnimator, sources},
    crate::project_watch::ProjectInputs,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(tag = "style", rename_all = "snake_case", deny_unknown_fields)]
pub enum Underline {
    Dotted {
        rgba: [u8; 4],
        diameter: u16,
        spacing: u16,
        offset: i16,
    },
}

impl Underline {
    pub(crate) fn validate(&self) -> Result<()> {
        let Self::Dotted {
            diameter, spacing, ..
        } = *self;
        ensure!(
            diameter > 0 && spacing >= diameter,
            "dotted underline requires positive diameter and spacing at least its diameter"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TextLine {
    /// Half-open UTF-8 range of the visible text, excluding trailing spaces.
    pub source_utf8: [usize; 2],
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub baseline: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TextMetrics {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<TextLine>,
    pub last_line_right_baseline: Option<[f64; 2]>,
    pub last_line_left_baseline: Option<[f64; 2]>,
}

#[cfg(feature = "host-audio")]
pub(crate) struct TextGeometry {
    /// Selectable rectangles in the sampled label's local coordinates.
    pub regions: Vec<[f64; 4]>,
    /// The paint surface, including the unused wrap width and animated offsets.
    pub raster_bounds: ([i32; 2], [u32; 2]),
}

/// Layout and font faces pinned to the same input observations as the document.
#[derive(Default)]
pub(crate) struct PreparedTextLayouts {
    text: HashMap<Content, Arc<PreparedText>>,
    layouts: HashMap<TextLayoutKey, Weak<text::ShapedLabel>>,
    faces: HashMap<PathBuf, Typeface>,
}

#[cfg(feature = "host-audio")]
impl PreparedTextLayouts {
    pub(crate) fn prepare(
        project: &mut Project,
        previous: Option<(&Self, &ProjectInputs)>,
        inputs: &ProjectInputs,
    ) -> Result<Self> {
        let mut cache = PaintCache::default();
        if let Some((old, observations)) = previous {
            cache.faces.extend(
                old.faces
                    .iter()
                    .filter(|(font, _)| inputs.same_file(observations, font))
                    .map(|(font, face)| (font.clone(), face.clone())),
            );
            cache.layouts.extend(
                old.layouts
                    .iter()
                    .filter(|(key, _)| cache.faces.contains_key(&key.font))
                    .map(|(key, shape)| (key.clone(), shape.clone())),
            );
        }
        let needed = sources(project.graphic_scopes());
        for content in needed.rasters.iter().chain(needed.animated_text.keys()) {
            let Content::Label { font, .. } = content else {
                continue;
            };
            if let Some(label) = previous
                .filter(|(_, observations)| inputs.same_file(observations, font))
                .and_then(|(old, _)| old.text.get(content))
            {
                cache.text.insert(content.clone(), Arc::clone(label));
            } else {
                cache.prepare_text(content, None, &mut PaintStats::default())?;
            }
        }
        cache.place_text_anchors(project, None)?;
        cache.retain(project);
        Ok(Self {
            text: cache.text,
            layouts: cache.layouts,
            faces: cache.faces,
        })
    }

    pub(crate) fn geometry(
        &self,
        content: &Content,
        animators: &[TextAnimator],
        frame: u32,
    ) -> Result<TextGeometry> {
        let Content::Label {
            text,
            font,
            size,
            width,
            rgb,
            underline,
        } = content
        else {
            anyhow::bail!("text geometry requires a label");
        };
        if let Some(layout) = self.text.get(content) {
            return layout.geometry(*rgb, animators, frame, underline.as_ref());
        }
        // A type-on sample can substitute a newly shaped prefix at this frame.
        // Both consumers shape that prefix with the accepted revision's face.
        ensure!(animators.is_empty(), "unprepared animated text");
        let face = self
            .faces
            .get(font)
            .context("unprepared label font")?
            .clone();
        text::ShapedLabel::new(&text.full_text(), font, face, *size, *width)?
            .selection_geometry(underline.as_ref(), None::<fn(_) -> _>)
    }
}

impl PaintCache {
    pub(crate) fn install_text_layouts(&mut self, layouts: &PreparedTextLayouts) {
        self.text.clone_from(&layouts.text);
        self.layouts.clone_from(&layouts.layouts);
        self.faces.clone_from(&layouts.faces);
    }
}

/// Font path is the resolved project asset. Coordinates are in the text box's local space.
pub fn text_metrics(layer: &Graphic) -> Result<TextMetrics> {
    let Content::Label {
        text: copy,
        font,
        size,
        width,
        ..
    } = &layer.content
    else {
        anyhow::bail!("text metrics requires a label");
    };
    ensure!(
        (1..=512).contains(size) && (1..=MAX_SURFACE_DIMENSION).contains(width),
        "invalid label dimensions"
    );
    Ok(text::ShapedLabel::new(
        &copy.full_text(),
        font,
        text::load_face(font)?,
        *size,
        *width,
    )?
    .metrics)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAnchorPoint {
    LastLineRightBaseline,
    LastLineLeftBaseline,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnchor {
    pub target: String,
    pub point: TextAnchorPoint,
    /// Derived project-local placement, refreshed before each accepted revision.
    #[serde(skip)]
    position: Option<[i32; 2]>,
}

impl TextAnchor {
    pub fn new(target: String, point: TextAnchorPoint) -> Self {
        Self {
            target,
            point,
            position: None,
        }
    }
}

// Authoring identity is stable before and after native layout preparation.
impl PartialEq for TextAnchor {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target && self.point == other.point
    }
}
impl Eq for TextAnchor {}
impl Hash for TextAnchor {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.target.hash(state);
        self.point.hash(state);
    }
}

impl Graphic {
    pub(crate) fn placed_position(&self) -> [i32; 2] {
        self.text_anchor
            .as_ref()
            .and_then(|a| a.position)
            .unwrap_or(self.position)
    }
}

fn target<'a>(layers: &'a [Graphic], name: &str) -> Result<&'a Graphic> {
    ensure!(!name.trim().is_empty(), "empty text anchor target");
    let mut candidates = layers.iter().filter(|g| g.name.as_deref() == Some(name));
    let label = candidates
        .next()
        .with_context(|| format!("missing sibling text anchor target {name}"))?;
    ensure!(
        candidates.next().is_none(),
        "ambiguous sibling text anchor target {name}"
    );
    ensure!(
        matches!(label.content, Content::Label { .. }),
        "text anchor target {name} must be a label"
    );
    ensure!(
        label.text_anchor.is_none()
            && label.type_on.is_none()
            && label.transform.is_none_or(|t| t.is_identity())
            && label
                .motion
                .iter()
                .all(|k| k.x == 0. && k.y == 0. && k.scale == 1.)
            && label.channels.as_ref().is_none_or(|c| !c.has_spatial()),
        "text anchor target {name} needs a fixed local layout; place shared motion on its parent"
    );
    Ok(label)
}

pub(super) fn validate_anchors(layers: &[Graphic]) -> Result<()> {
    for a in layers.iter().filter_map(|l| l.text_anchor.as_ref()) {
        target(layers, &a.target)?;
    }
    Ok(())
}

impl PaintCache {
    pub(crate) fn place_text_anchors(
        &mut self,
        project: &mut Project,
        accepted: Option<&Self>,
    ) -> Result<()> {
        let mut has_anchors = false;
        for layers in project.graphic_scopes_mut() {
            has_anchors |= self.place_in_scope(layers, accepted)?;
        }
        if has_anchors {
            project.validate()?;
        }
        Ok(())
    }

    fn place_in_scope(&mut self, layers: &mut [Graphic], accepted: Option<&Self>) -> Result<bool> {
        let mut has_anchors = false;
        for index in 0..layers.len() {
            if let Some(anchor) = &layers[index].text_anchor {
                has_anchors = true;
                let label = target(layers, &anchor.target)?;
                self.prepare_text(&label.content, accepted, &mut PaintStats::default())?;
                let lines = &self.text[&label.content].shape.metrics.lines;
                let at = lines.last().map_or([0., 0.], |line| {
                    let x = match anchor.point {
                        TextAnchorPoint::LastLineLeftBaseline => line.left,
                        TextAnchorPoint::LastLineRightBaseline => line.right,
                    };
                    [x, line.baseline]
                });
                let mut resolved = [0; 2];
                for axis in 0..2 {
                    let value = i64::from(label.position[axis])
                        + at[axis].round() as i64
                        + i64::from(layers[index].position[axis]);
                    resolved[axis] =
                        i32::try_from(value).context("text anchor exceeds coordinate range")?;
                }
                layers[index].text_anchor.as_mut().unwrap().position = Some(resolved);
            }
            if let Content::Group { children } | Content::TransformGroup { children } =
                &mut layers[index].content
            {
                has_anchors |= self.place_in_scope(children, accepted)?;
            } else if let Content::Scene3d(scene) = &mut layers[index].content {
                for surface in scene.models.iter_mut().flat_map(|m| &mut m.surfaces) {
                    has_anchors |= self.place_in_scope(&mut surface.layers, accepted)?;
                }
            }
        }
        Ok(has_anchors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{LabelText, validate};
    use serde_json::json;
    use std::path::Path;

    fn listing(copy: &str) -> Result<Project> {
        let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf");
        Ok(serde_json::from_value(json!({
            "size":[320,200], "fps":[30,1], "shots":[{"duration":3,"background":{"color":[0,0,0]},
            "graphics":[{"name":"Badge","position":[5,-2],"text_anchor":{"target":"Venue","point":"last_line_right_baseline"},
            "content":{"type":"path","d":"M0 0h5v5h-5z","fill":[255,0,0,255]}},
            {"name":"Venue","position":[11,7],"content":{"type":"label","text":copy,"font":font,"size":24,"width":138,
            "rgb":[0,0,255],"underline":{"style":"dotted","diameter":4,"spacing":8,"offset":3,"rgba":[0,255,0,255]}}}]}]
        }))?)
    }

    #[test]
    fn dots_use_shaped_lines_and_report_utf8_ranges() -> Result<()> {
        let project = listing("Café Hall\nPark")?;
        let label = &project.shots[0].graphics[1];
        let mut cache = PaintCache::default();
        let report = text_metrics(label)?;
        assert_eq!(report.lines.len(), 2);
        let texts: Vec<_> = report
            .lines
            .iter()
            .map(|l| &"Café Hall\nPark"[l.source_utf8[0]..l.source_utf8[1]])
            .collect();
        assert_eq!(texts, ["Café Hall", "Park"]);
        let raster = cache.raster(&label.content, &mut PaintStats::default())?;
        let green = |x: usize, y: usize| {
            if x < raster.size[0] as usize {
                raster.rgba[(y * raster.size[0] as usize + x) * 4 + 1]
            } else {
                0
            }
        };
        for line in &report.lines {
            let y = ((line.baseline + 3.).floor() as i32 - raster.origin[1]) as usize;
            assert!(green(1, y) >= 220 && green(9, y) >= 220);
            assert_eq!(green(5, y), 0, "gap between dots");
            assert_eq!(green(line.right.ceil() as usize + 1, y), 0);
        }
        for copy in ["", "a\n\n", "  "] {
            let p = listing(copy)?;
            let m = text_metrics(&p.shots[0].graphics[1])?;
            for l in m.lines {
                assert!(
                    copy.get(l.source_utf8[0]..l.source_utf8[1]).is_some(),
                    "invalid byte range {:?} for {copy:?}",
                    l.source_utf8
                );
            }
        }
        Ok(())
    }

    #[test]
    fn sibling_anchor_is_unambiguous_local_and_acyclic() -> Result<()> {
        let p = listing("Park")?;
        let layers = p.shots[0].graphics.clone();
        let mut placed = p.clone();
        PaintCache::default().place_text_anchors(&mut placed, None)?;
        assert_eq!(
            p, placed,
            "preparation must preserve authored project identity"
        );
        let mut duplicate = layers.clone();
        duplicate.push(layers[1].clone());
        assert!(
            validate(&duplicate, 3)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(validate(&layers[..1], 3).is_err());
        let mut invalid = layers.clone();
        invalid[1].text_anchor = layers[0].text_anchor.clone();
        assert!(validate(&invalid, 3).is_err());
        invalid[1].text_anchor = None;
        invalid[1].type_on = Some(vec![(0, Some(1))]);
        assert!(validate(&invalid, 3).is_err());
        let mut overflowing = p;
        overflowing.shots[0].graphics[0].position = [i32::MAX, 0];
        assert!(
            PaintCache::default()
                .place_text_anchors(&mut overflowing, None)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn one_copy_edit_repositions_the_badge_and_reverses_cleanly() -> Result<()> {
        let short = listing("Park")?;
        let mut engine = crate::Engine::new(short.clone())?;
        let before = engine.render(0)?.rgba;
        let check = |engine: &crate::Engine, rgba: &[u8]| -> Result<(i32, i32)> {
            let layers = &engine.project().shots[0].graphics;
            let label = &layers[1];
            let measured = text_metrics(label)?;
            let [end, baseline] = measured.last_line_right_baseline.unwrap();
            let x = 11 + end.round() as i32 + 5;
            let y = 7 + baseline.round() as i32 - 2;
            assert_eq!(layers[0].placed_position(), [x, y]);
            let pixel =
                |x, y| &rgba[((320 * y + x) * 4) as usize..((320 * y + x) * 4 + 4) as usize];
            assert_eq!(pixel(x + 2, y + 2), &[255, 0, 0, 255]);
            Ok((x, y))
        };
        let (_, y1) = check(&engine, &before)?;
        let mut long = short.clone();
        if let Content::Label { text, .. } = &mut long.shots[0].graphics[1].content {
            *text = LabelText::Plain("Station visitor parking".into());
        }
        engine.replace_project(long)?;
        let revised = engine.render(2)?.rgba;
        let (_, y2) = check(&engine, &revised)?;
        assert!(y2 > y1 + 20);
        assert_eq!(engine.render(0)?.rgba, revised);
        engine.replace_project(short)?;
        assert_eq!(engine.render(1)?.rgba, before);
        Ok(())
    }
}
