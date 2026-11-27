//! Authored logical ranges and their evaluated paint on fully shaped text.
use super::ScalarKey;
use crate::render::channels::{sample_scalar, validate_keys};
use crate::render::text::{GlyphStyle, ShapedLabel};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::HashMap, ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LabelText {
    Plain(String),
    Spans(Vec<TextSpan>),
}

impl LabelText {
    pub(super) fn full_text(&self) -> Cow<'_, str> {
        match self {
            Self::Plain(text) => Cow::Borrowed(text),
            Self::Spans(spans) => Cow::Owned(spans.iter().map(|s| s.text.as_str()).collect()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextSpan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnimator {
    /// A named span, or the entire headline if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Vec<ScalarKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<[i32; 2]>,
}

pub(super) struct TextIndex {
    pub source: Arc<str>,
    /// UTF-8 byte offsets including the end of the source string.
    boundaries: Vec<usize>,
    spans: HashMap<String, Range<usize>>,
}

impl TextIndex {
    pub fn new(copy: &LabelText) -> Result<Self> {
        let source = copy.full_text().into_owned();
        let boundaries: Vec<_> = source
            .grapheme_indices(true)
            .map(|(at, _)| at)
            .chain(std::iter::once(source.len()))
            .collect();
        let mut spans = HashMap::new();
        if let LabelText::Spans(parts) = copy {
            let mut start = 0;
            for part in parts {
                let end = start + part.text.len();
                if let Some(id) = &part.id {
                    ensure!(!id.trim().is_empty(), "empty text span id");
                    let range = boundaries
                        .binary_search(&start)
                        .ok()
                        .zip(boundaries.binary_search(&end).ok())
                        .with_context(|| format!("text span {id} splits a grapheme"))?;
                    ensure!(
                        spans.insert(id.clone(), range.0..range.1).is_none(),
                        "duplicate text span {id}"
                    );
                }
                start = end;
            }
        }
        Ok(Self {
            source: source.into(),
            boundaries,
            spans,
        })
    }

    fn units(&self, target: Option<&str>) -> Result<Range<usize>> {
        target.map_or_else(
            || Ok(0..self.boundaries.len() - 1),
            |id| {
                self.spans
                    .get(id)
                    .cloned()
                    .with_context(|| format!("unknown text span {id}"))
            },
        )
    }

    pub fn sample(
        &self,
        animators: &[TextAnimator],
        frame: u32,
        fraction: f64,
    ) -> Result<Vec<SampledRange>> {
        let mut ranges = Vec::with_capacity(animators.len());
        for animator in animators {
            let units = self.units(animator.target.as_deref())?;
            let at = |track: &Option<Vec<ScalarKey>>, default| {
                let normalized = track
                    .as_ref()
                    .map_or(default, |keys| sample_scalar(keys, frame, fraction));
                let count = (f64::from(normalized) * units.len() as f64).round() as usize;
                self.boundaries[units.start + count]
            };
            let bytes = at(&animator.start, 0.)..at(&animator.end, 1.);
            if !bytes.is_empty() {
                ranges.push(SampledRange {
                    bytes,
                    fill: animator.fill,
                    offset: animator.offset,
                });
            }
        }
        Ok(ranges)
    }
}

impl TextAnimator {
    pub(super) fn validate(&self, index: &TextIndex) -> Result<()> {
        index.units(self.target.as_deref())?;
        ensure!(
            self.fill.is_some() || self.offset.is_some(),
            "text animator requires fill or offset"
        );
        for (name, track) in [
            ("text range start", &self.start),
            ("text range end", &self.end),
        ] {
            if let Some(keys) = track {
                validate_keys(name, keys)?;
                ensure!(
                    keys.iter()
                        .all(|key| (0.0..=1.0).contains(&f32::from(key.value))),
                    "{name} must be between 0 and 1"
                );
            }
        }
        Ok(())
    }
}

/// Envelope for any combination of selected offsets, including the neutral bounds.
pub(super) fn offset_bounds(
    animators: &[TextAnimator],
    size: [u32; 2],
) -> Result<([i32; 2], [u32; 2])> {
    // Distinct named spans contain disjoint units. Only global effects and
    // effects targeting the same span can accumulate on one unit.
    let mut spans = HashMap::new();
    for effect in animators {
        let Some(offset) = effect.offset else {
            continue;
        };
        let (lo, hi) = spans
            .entry(effect.target.as_deref())
            .or_insert(([0i64; 2], [0i64; 2]));
        for axis in 0..2 {
            let delta = i64::from(offset[axis]);
            lo[axis] += delta.min(0);
            hi[axis] += delta.max(0);
        }
    }
    let (global_min, global_max) = spans.remove(&None).unwrap_or_default();
    let mut minimum = global_min;
    let mut maximum = global_max;
    for (lo, hi) in spans.values() {
        for axis in 0..2 {
            minimum[axis] = minimum[axis].min(global_min[axis] + lo[axis]);
            maximum[axis] = maximum[axis].max(global_max[axis] + hi[axis]);
        }
    }
    let extent = [0, 1].map(|axis| i64::from(size[axis]) + maximum[axis] - minimum[axis]);
    ensure!(
        extent
            .iter()
            .all(|&n| n <= i64::from(crate::MAX_SURFACE_DIMENSION)),
        "text animator offset exceeds maximum raster extent"
    );
    Ok((minimum.map(|n| n as i32), extent.map(|n| n as u32)))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SampledRange {
    bytes: Range<usize>,
    fill: Option<[u8; 4]>,
    offset: Option<[i32; 2]>,
}

pub(super) struct PreparedText {
    pub index: TextIndex,
    pub shape: Arc<ShapedLabel>,
}

impl PreparedText {
    pub fn paint(
        &self,
        rgb: [u8; 3],
        ranges: &[SampledRange],
        underline: Option<&super::Underline>,
    ) -> Result<super::Raster> {
        self.shape.paint_decorated(
            |bytes| glyph_style(&self.index.boundaries, rgb, ranges, bytes),
            underline,
        )
    }

    #[cfg(feature = "host-audio")]
    pub(super) fn geometry(
        &self,
        rgb: [u8; 3],
        animators: &[TextAnimator],
        frame: u32,
        underline: Option<&super::Underline>,
    ) -> Result<super::TextGeometry> {
        let ranges = self.index.sample(animators, frame, 0.)?;
        self.shape.selection_geometry(
            underline,
            (!ranges.is_empty())
                .then_some(|bytes| glyph_style(&self.index.boundaries, rgb, &ranges, bytes)),
        )
    }
}

fn glyph_style(
    boundaries: &[usize],
    rgb: [u8; 3],
    ranges: &[SampledRange],
    cluster: Range<usize>,
) -> GlyphStyle {
    let base = GlyphStyle {
        rgba: [rgb[0], rgb[1], rgb[2], 255],
        offset: [0.; 2],
    };
    let mut result = base;
    let mut alpha = u8::MAX;
    let start = boundaries
        .partition_point(|&at| at <= cluster.start)
        .saturating_sub(1);
    // A ligature takes its final logical unit's tint and offset, and all its
    // units must be revealed before its shared glyph becomes visible.
    for &unit in boundaries[start..]
        .iter()
        .take_while(|&&at| at < cluster.end)
    {
        let mut style = base;
        for range in ranges.iter().filter(|r| r.bytes.contains(&unit)) {
            if let Some(fill) = range.fill {
                style.rgba = fill;
            }
            if let Some([dx, dy]) = range.offset {
                style.offset[0] += dx as f32;
                style.offset[1] += dy as f32;
            }
        }
        alpha = alpha.min(style.rgba[3]);
        result = style;
    }
    result.rgba[3] = alpha;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Engine, Project,
        graphics::{Content, Graphic},
    };
    use serde_json::{Value, json};
    use std::path::Path;

    #[test]
    fn range_selection_uses_logical_graphemes_and_the_whole_cluster() -> Result<()> {
        let copy =
            serde_json::from_value(json!([{"text":"א "},{"id":"word","text":"a\u{301}ffi"}]))?;
        let index = TextIndex::new(&copy)?;
        let effects: Vec<TextAnimator> = serde_json::from_value(json!([
            {"fill":[0,0,0,0]},
            {"target":"word","fill":[20,40,60,255],"end":[{"frame":0,"value":0.25,"ease":"hold"},
                                                         {"frame":3,"value":0.75},{"frame":5,"value":1}]}
        ]))?;
        for effect in &effects {
            effect.validate(&index)?;
        }
        let word = index.source.find('a').unwrap();
        let ffi = index.source.find('f').unwrap()..index.source.len();
        let colors = |frame, bytes| -> Result<_> {
            let ranges = index.sample(&effects, frame, 0.)?;
            Ok(glyph_style(&index.boundaries, [1; 3], &ranges, bytes).rgba)
        };
        assert_eq!(colors(2, word..ffi.start)?, [20, 40, 60, 255]);
        assert_eq!(colors(3, ffi.clone())?[3], 0);
        assert_eq!(colors(5, ffi)?, [20, 40, 60, 255]);
        assert_eq!(colors(5, 0..2)?[3], 0);
        Ok(())
    }

    #[test]
    fn validates_span_identity_selector_curves_and_offset_limits() -> Result<()> {
        let layer = |text: Value, animator: Value| -> Result<()> {
            let layer: Graphic = serde_json::from_value(json!({
                "content":{"type":"label","text":text,"font":"face.ttf","size":16,"width":80,"rgb":[0,0,0]},
                "text_animators":[animator]
            }))?;
            super::super::validate(&[layer], 10)
        };
        let copy = json!([{"id":"x", "text":"x"}]);
        let fx = json!({"target":"x","offset":[0,-8]});
        layer(copy.clone(), fx.clone())?;
        for invalid in [
            json!({"target":"absent","fill":[0,0,0,255]}),
            json!({"end":[],"fill":[0,0,0,255]}),
            json!({"end":[{"frame":1,"value":1.1}],"fill":[0,0,0,255]}),
            json!({"start":[{"frame":3,"value":0},{"frame":2,"value":1}],"offset":[0,2]}),
            json!({"offset":[2147483647,0]}),
            json!({"target":"x"}),
        ] {
            assert!(layer(copy.clone(), invalid).is_err());
        }
        for invalid in [
            json!([{"id":"x","text":"a"},{"id":"x","text":"b"}]),
            json!([{"id":"x","text":"a"},{"text":"\u{301}"}]),
        ] {
            assert!(layer(invalid, fx.clone()).is_err());
        }
        Ok(())
    }

    #[test]
    fn changing_range_paint_and_copy_reuses_only_valid_layouts_and_pictures() -> Result<()> {
        let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf");
        let mut project: Project = serde_json::from_value(json!({
            "size":[160,64],"fps":[30,1],"shots":[{"duration":4,"background":{"color":[0,0,0]},
            "graphics":[{"position":[8,14],"content":{"type":"label","text":[{"text":"L  "},{"id":"target","text":"M"},{"text":"  R"}],
                "font":font,"size":26,"width":145,"rgb":[255,255,255]},
            "text_animators":[{"target":"target","fill":[255,0,0,255]},
                {"target":"target","offset":[0,-8],"end":[{"frame":0,"value":0,"ease":"hold"},{"frame":1,"value":1}]}]}]}]
        }))?;
        let mut engine = Engine::new(project.clone())?;
        let first = engine.render(0)?;
        assert_eq!(first.trace.graphics.text_layouts, 1);
        let moved = engine.render(2)?;
        assert_eq!(moved.trace.graphics.text_layouts, 0);
        let colors = |rgba: &[u8]| {
            let mut glyph = Vec::new();
            let mut neighbors = Vec::new();
            for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
                let x = i % 160;
                let y = i / 160;
                if px[0] > px[1] {
                    glyph.push((x, y as i32, px[0]));
                } else if px[1] > 0 {
                    neighbors.push((x, y, px[1]));
                }
            }
            (glyph, neighbors)
        };
        let (before, neighbors) = colors(&first.rgba);
        let (after, still) = colors(&moved.rgba);
        assert!(!before.is_empty() && !neighbors.is_empty());
        assert_eq!(
            after,
            before
                .into_iter()
                .map(|(x, y, r)| (x, y - 8, r))
                .collect::<Vec<_>>()
        );
        assert_eq!(neighbors, still);
        let reverse = engine.render(0)?;
        assert_eq!(first.rgba, reverse.rgba);
        assert_eq!(reverse.trace.graphics.raster_paints, 0);
        let original_geometry = engine.graphics.text.values().next().unwrap().shape.clone();

        project.shots[0].graphics[0].text_animators[0].fill = Some([0, 255, 0, 255]);
        engine.replace_project(project.clone())?;
        assert_ne!(engine.render(0)?.rgba, first.rgba);
        if let Content::Label { rgb, .. } = &mut project.shots[0].graphics[0].content {
            *rgb = [180; 3];
        }
        engine.replace_project(project.clone())?;
        assert!(Arc::ptr_eq(
            &engine.graphics.text.values().next().unwrap().shape,
            &original_geometry
        ));

        let saved = project.clone();
        if let Content::Label { font, .. } = &mut project.shots[0].graphics[0].content {
            *font = "absent.ttf".into();
        }
        assert!(engine.replace_project(project.clone()).is_err());
        assert_eq!(engine.project(), &saved);
        project = saved;
        if let Content::Label {
            text: LabelText::Spans(spans),
            ..
        } = &mut project.shots[0].graphics[0].content
        {
            spans[1].text = "MN".into();
        }
        engine.replace_project(project)?;
        assert!(!Arc::ptr_eq(
            &engine.graphics.text.values().next().unwrap().shape,
            &original_geometry
        ));
        assert_eq!(engine.graphics.text.len(), 1);
        assert!(engine.graphics.samples.is_empty());
        let revised = engine.render(0)?;
        assert_eq!(
            revised.trace.graphics.text_layouts, 0,
            "layout already staged with revision"
        );
        assert_eq!(revised.trace.graphics.raster_paints, 1);
        Ok(())
    }
}
