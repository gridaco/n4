//! Nominal-frame geometry in composition pixels, using the painter's evaluated spaces.
mod coverage;

use crate::{
    Project,
    graphics::{Content, Graphic, SampleTime, uses_footage},
    inspect::{Inspection, Placement},
    project_watch::InputOwner,
    render::{LayerSample, affine::LayerSpace, sample_layer},
};
use anyhow::{Context, Result, ensure};
use coverage::{
    Bounds, Clip, Coverage, Point, bounds_of, contains, corners, evaluate_leaf, has_area,
    intersect_polygon, point_coverage, sample_clip, uncertain_paint,
};
pub(super) use coverage::{GeometryAssets, NativeCoverageCache};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Address of an authored drawable within a single accepted document revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerRef {
    pub occurrence: String,
    /// Zero-based indexes through authored sibling lists, including groups.
    pub layers: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Visible,
    Disabled,
    Inactive,
    Empty,
    Indeterminate,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerGeometry {
    pub target: LayerRef,
    pub visibility: Visibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_frame: Option<u32>,
    /// Left, top, right, bottom, in authored layer pixels before masking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_bounds: Option<[f64; 4]>,
    /// Two affine rows mapping a homogeneous local point into the picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_to_composition: Option<[[f64; 3]; 2]>,
    /// Transformed bounds ordered clockwise from the local top left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composition_quad: Option<[[f64; 2]; 4]>,
    /// Local geometric clip bounds from ancestors through the layer itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<GeometryClip>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeometryClip {
    pub local_to_composition: [[f64; 3]; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_bounds: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wipe_right: Option<f64>,
    pub empty: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HitTest {
    /// Known layers in front-to-back paint order, stopping at unknown coverage.
    pub hits: Vec<LayerRef>,
    pub indeterminate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obstruction: Option<LayerRef>,
    /// The primary shot placement when a point reaches its implicit footage.
    #[serde(rename = "fallbackPlacement", skip_serializing_if = "Option::is_none")]
    pub fallback_placement: Option<String>,
}

fn layers_at<'a>(project: &'a Project, placement: &Placement) -> Option<&'a [Graphic]> {
    match placement.owner {
        Some(InputOwner::Shot(index)) => Some(&project.shots.get(index)?.graphics),
        Some(InputOwner::Overlay(index)) => Some(&project.overlays.get(index)?.graphics),
        None if placement.path == "V2:graphics@0" => Some(&project.graphics),
        _ => None,
    }
}

fn children(layer: &Graphic) -> &[Graphic] {
    match &layer.content {
        Content::Group { children } | Content::TransformGroup { children } => children,
        _ => &[],
    }
}

fn layer_source<'a>(
    project: &'a Project,
    inspection: &Inspection,
    layer: &LayerRef,
) -> Option<&'a Graphic> {
    let placement = inspection.placement(&layer.occurrence)?;
    let mut siblings = layers_at(project, placement)?;
    let (last, parents) = layer.layers.split_last()?;
    for &index in parents {
        siblings = children(siblings.get(index)?);
    }
    siblings.get(*last)
}

pub fn layer_exists(project: &Project, inspection: &Inspection, layer: &LayerRef) -> bool {
    layer_source(project, inspection, layer).is_some()
}

/// An ordinal can be rebound to an unchanged branch when each selected child
/// is distinguishable from its siblings.
pub(super) fn unique_branch<'a>(
    project: &'a Project,
    inspection: &Inspection,
    layer: &LayerRef,
) -> Option<&'a Graphic> {
    let mut siblings = layers_at(project, inspection.placement(&layer.occurrence)?)?;
    let root = siblings.get(*layer.layers.first()?)?;
    for &index in &layer.layers {
        let selected = siblings.get(index)?;
        if siblings
            .iter()
            .filter(|sibling| *sibling == selected)
            .take(2)
            .count()
            != 1
        {
            return None;
        }
        siblings = children(selected);
    }
    Some(root)
}

fn root_space(project: &Project, owner: Option<InputOwner>, clock: u32) -> LayerSpace {
    let transform = match owner {
        Some(InputOwner::Shot(index)) => project.shots[index]
            .camera_at(clock)
            .and_then(|camera| camera.affine(SampleTime::at(clock, None, 0), project.size)),
        _ => None,
    };
    LayerSpace {
        origin: [0; 2],
        transform,
    }
}

fn content_in_scope(
    layer: &Graphic,
    local_frame: u32,
    scope_clock: u32,
) -> Option<Cow<'_, Content>> {
    let content = layer.content_at(local_frame, 0.)?;
    // Video's source interval uses the paint scope's clock, even under a layer span.
    if let Content::Video(clip) = content.as_ref() {
        clip.source_frame(scope_clock)?;
    }
    Some(content)
}

pub(super) fn resolve(
    project: &Project,
    inspection: &Inspection,
    assets: GeometryAssets<'_>,
    frame: u32,
    target: &LayerRef,
) -> Result<LayerGeometry> {
    ensure!(frame < inspection.duration, "frame outside project");
    ensure!(
        layer_exists(project, inspection, target),
        "unknown layer occurrence"
    );
    let placement = inspection.placement(&target.occurrence).unwrap();
    let mut view = LayerGeometry {
        target: target.clone(),
        visibility: Visibility::Inactive,
        local_frame: None,
        local_bounds: None,
        local_to_composition: None,
        composition_quad: None,
        clips: Vec::new(),
    };
    if !placement.enabled {
        view.visibility = Visibility::Disabled;
        return Ok(view);
    }
    let [start, end] = placement.project;
    if !(start..end).contains(&frame) {
        return Ok(view);
    }
    let mut siblings = layers_at(project, placement).unwrap();
    let scope_clock = frame - start;
    let mut clock = scope_clock;
    let mut space = root_space(project, placement.owner, clock);
    let mut clips = Vec::new();
    let mut uncertain = false;
    let (last, parents) = target.layers.split_last().unwrap();
    for &index in parents {
        let layer = &siblings[index];
        let Some(sample) = sample_layer(layer, space, clock, 0.) else {
            return Ok(view);
        };
        if sample.opacity == 0. {
            view.visibility = Visibility::Empty;
            return Ok(view);
        }
        if matches!(layer.content, Content::Group { .. }) {
            let clip = sample_clip(
                layer,
                &sample,
                scope_clock,
                ([0; 2], project.size),
                assets.rasters,
            )?;
            if clip.constrained() {
                clips.push(clip);
            }
            uncertain |= uncertain_paint(layer);
        }
        siblings = children(layer);
        clock = sample.frame;
        space = sample.space;
    }
    let layer = &siblings[*last];
    let Some(sample) = sample_layer(layer, space, clock, 0.) else {
        return Ok(view);
    };
    let Some(content) = content_in_scope(layer, sample.frame, scope_clock) else {
        return Ok(view);
    };
    view.local_frame = Some(sample.frame);
    view.local_to_composition = Some(sample.space.rows());
    let leaf = evaluate_leaf(layer, &content, sample.frame, assets)?;
    let raster = if matches!(*content, Content::Group { .. }) {
        Some(([0; 2], project.size))
    } else {
        leaf.as_ref().map(|leaf| leaf.raster)
    };
    if let Some(raster) = raster {
        let clip = sample_clip(layer, &sample, scope_clock, raster, assets.rasters)?;
        if clip.constrained() {
            clips.push(clip);
        }
    }
    view.clips = clips.iter().map(Clip::geometry).collect();
    if sample.opacity == 0. {
        view.visibility = Visibility::Empty;
        return Ok(view);
    }
    uncertain |= uncertain_paint(layer);
    let local_bounds = match &leaf {
        Some(leaf) => leaf.bounds(),
        None if matches!(
            *content,
            Content::Group { .. } | Content::TransformGroup { .. }
        ) =>
        {
            let mut extents = None;
            if !collect_group_bounds(
                children(layer),
                &sample,
                sample.space,
                scope_clock,
                assets,
                &mut extents,
            )? {
                view.visibility = Visibility::Indeterminate;
                return Ok(view);
            }
            extents
        }
        None => {
            view.visibility = Visibility::Indeterminate;
            return Ok(view);
        }
    };
    let Some(local_bounds) = local_bounds else {
        view.visibility = Visibility::Empty;
        return Ok(view);
    };
    let quad = corners(local_bounds).map(|p| sample.space.to_composition(p));
    view.local_bounds = Some(local_bounds);
    view.composition_quad = Some(quad);
    let intersects = |region| {
        let polygon = corners(region)
            .map(|p| sample.space.to_composition(p))
            .to_vec();
        has_area(
            &clips
                .iter()
                .fold(polygon, |polygon, clip| clip.shape(polygon)),
        )
    };
    let intersects = leaf.as_ref().map_or_else(
        || intersects(local_bounds),
        |leaf| leaf.regions().any(intersects),
    );
    view.visibility = if uncertain {
        Visibility::Indeterminate
    } else if intersects {
        Visibility::Visible
    } else {
        Visibility::Empty
    };
    Ok(view)
}

fn collect_group_bounds(
    layers: &[Graphic],
    group: &LayerSample,
    target_space: LayerSpace,
    scope_clock: u32,
    assets: GeometryAssets<'_>,
    extents: &mut Option<Bounds>,
) -> Result<bool> {
    for layer in layers {
        let Some(sample) = sample_layer(layer, group.space, group.frame, 0.) else {
            continue;
        };
        if sample.opacity == 0. {
            continue;
        }
        let Some(content) = content_in_scope(layer, sample.frame, scope_clock) else {
            continue;
        };
        match content.as_ref() {
            Content::Group { children } | Content::TransformGroup { children } => {
                if !collect_group_bounds(
                    children,
                    &sample,
                    target_space,
                    scope_clock,
                    assets,
                    extents,
                )? {
                    return Ok(false);
                }
            }
            _ => {
                let Some(leaf) = evaluate_leaf(layer, &content, sample.frame, assets)? else {
                    return Ok(false);
                };
                if let Some(rect) = leaf.bounds() {
                    let points = corners(rect)
                        .map(|p| target_space.to_local(sample.space.to_composition(p)));
                    *extents = bounds_of(extents.iter().copied().flat_map(corners).chain(points));
                }
            }
        }
    }
    Ok(true)
}

fn find_owner(placements: &[Placement], owner: InputOwner) -> Option<&Placement> {
    placements.iter().find_map(|placement| {
        if placement.owner == Some(owner) {
            Some(placement)
        } else {
            find_owner(&placement.children, owner)
        }
    })
}

enum PickArea {
    Point { at: Point },
    Marquee(Vec<Point>),
}

struct Picker<'a> {
    assets: GeometryAssets<'a>,
    front_only: bool,
    result: HitTest,
}

impl Picker<'_> {
    fn finished(&self) -> bool {
        self.result.indeterminate || self.front_only && !self.result.hits.is_empty()
    }

    fn report(&mut self, coverage: Coverage, occurrence: &str, path: &[usize]) {
        let target = || LayerRef {
            occurrence: occurrence.into(),
            layers: path.to_vec(),
        };
        match coverage {
            Coverage::Outside => {}
            Coverage::Inside => self.result.hits.push(target()),
            Coverage::Unknown => {
                self.result.indeterminate = true;
                self.result.obstruction = Some(target());
            }
        }
    }

    fn layers(
        &mut self,
        occurrence: &str,
        layers: &[Graphic],
        path: &mut Vec<usize>,
        space: LayerSpace,
        clock: u32,
        scope_clock: u32,
        area: &PickArea,
    ) -> Result<()> {
        for (index, layer) in layers.iter().enumerate().rev() {
            let Some(sample) = sample_layer(layer, space, clock, 0.) else {
                continue;
            };
            if sample.opacity == 0. {
                continue;
            }
            let Some(content) = content_in_scope(layer, sample.frame, scope_clock) else {
                continue;
            };

            path.push(index);
            match content.as_ref() {
                Content::TransformGroup { children } => {
                    self.layers(
                        occurrence,
                        children,
                        path,
                        sample.space,
                        sample.frame,
                        scope_clock,
                        area,
                    )?;
                }
                Content::Group { children } => {
                    let clip = sample_clip(
                        layer,
                        &sample,
                        scope_clock,
                        ([0; 2], self.assets.canvas),
                        self.assets.rasters,
                    )?;
                    let clipped;
                    let area = match area {
                        PickArea::Point { at }
                            if layer.underlay.is_none() && clip.constrained() =>
                        {
                            clip.contains(*at, self.assets)?.then_some(area)
                        }
                        PickArea::Marquee(polygon) if clip.constrained() => {
                            let polygon = clip.shape(polygon.clone());
                            let intersects = has_area(&polygon);
                            clipped = PickArea::Marquee(polygon);
                            intersects.then_some(&clipped)
                        }
                        _ => Some(area),
                    };
                    if let Some(area) = area {
                        if !uncertain_paint(layer) {
                            self.layers(
                                occurrence,
                                children,
                                path,
                                sample.space,
                                sample.frame,
                                scope_clock,
                                area,
                            )?;
                        } else if matches!(area, PickArea::Point { .. }) {
                            self.report(Coverage::Unknown, occurrence, path);
                        }
                    }
                }
                _ => {
                    let coverage =
                        self.leaf_coverage(layer, &content, &sample, scope_clock, area)?;
                    self.report(coverage, occurrence, path);
                }
            }
            path.pop();
            if self.finished() {
                break;
            }
        }
        Ok(())
    }

    fn leaf_coverage(
        &self,
        layer: &Graphic,
        content: &Content,
        sample: &LayerSample,
        scope_clock: u32,
        area: &PickArea,
    ) -> Result<Coverage> {
        if layer.underlay.is_some() && matches!(area, PickArea::Point { .. }) {
            // A shadow can extend beyond either the source or its clip.
            return Ok(Coverage::Unknown);
        }
        let Some(leaf) = evaluate_leaf(layer, content, sample.frame, self.assets)? else {
            return Ok(match area {
                PickArea::Point { .. } => Coverage::Unknown,
                PickArea::Marquee(_) => Coverage::Outside,
            });
        };
        let clip = sample_clip(layer, sample, scope_clock, leaf.raster, self.assets.rasters)?;
        match area {
            PickArea::Point { at } => {
                point_coverage(layer, content, &leaf, &clip, *at, self.assets)
            }
            PickArea::Marquee(polygon) => {
                if uncertain_paint(layer) {
                    return Ok(Coverage::Outside);
                }
                let polygon = clip.shape(polygon.clone());
                let hit = leaf.regions().any(|region| {
                    has_area(&intersect_polygon(polygon.clone(), sample.space, region))
                });
                Ok(if hit {
                    Coverage::Inside
                } else {
                    Coverage::Outside
                })
            }
        }
    }

    fn scope(
        &mut self,
        layers: &[Graphic],
        placement: Option<&Placement>,
        space: LayerSpace,
        clock: u32,
        area: &PickArea,
    ) -> Result<()> {
        if layers.is_empty() || self.finished() {
            return Ok(());
        }
        if let Some(placement) = placement {
            self.layers(
                &placement.path,
                layers,
                &mut Vec::new(),
                space,
                clock,
                clock,
                area,
            )?;
        } else if matches!(area, PickArea::Point { .. }) {
            self.result.indeterminate = true;
        }
        Ok(())
    }
}

pub(super) fn hit_test(
    project: &Project,
    inspection: &Inspection,
    assets: GeometryAssets<'_>,
    frame: u32,
    point: Point,
) -> Result<HitTest> {
    point_hits(project, inspection, assets, frame, point, false)
}

pub(super) fn hover_target(
    project: &Project,
    inspection: &Inspection,
    assets: GeometryAssets<'_>,
    frame: u32,
    point: Point,
) -> Result<Option<LayerRef>> {
    Ok(point_hits(project, inspection, assets, frame, point, true)?
        .hits
        .into_iter()
        .next())
}

fn point_hits(
    project: &Project,
    inspection: &Inspection,
    assets: GeometryAssets<'_>,
    frame: u32,
    point: Point,
    front_only: bool,
) -> Result<HitTest> {
    ensure!(
        point.into_iter().all(f64::is_finite),
        "pick point must be finite"
    );
    if point
        .into_iter()
        .zip(project.size)
        .any(|(p, limit)| p < 0. || p >= f64::from(limit))
    {
        return Ok(HitTest::default());
    }
    pick(
        project,
        inspection,
        assets,
        frame,
        &PickArea::Point { at: point },
        front_only,
    )
}

pub(super) fn marquee_hits(
    project: &Project,
    inspection: &Inspection,
    assets: GeometryAssets<'_>,
    frame: u32,
    area: Bounds,
) -> Result<Vec<LayerRef>> {
    if area[0] >= area[2] || area[1] >= area[3] {
        return Ok(Vec::new());
    }
    Ok(pick(
        project,
        inspection,
        assets,
        frame,
        &PickArea::Marquee(corners(area).to_vec()),
        false,
    )?
    .hits)
}

fn pick(
    project: &Project,
    inspection: &Inspection,
    assets: GeometryAssets<'_>,
    frame: u32,
    area: &PickArea,
    front_only: bool,
) -> Result<HitTest> {
    let (shot, shot_clock) = project.shot_at(frame)?;
    let mut picker = Picker {
        assets,
        front_only,
        result: HitTest::default(),
    };
    if matches!(area, PickArea::Point { .. })
        && (!project.title.text.is_empty() || project.title.backplate.is_some())
        && project.request(frame)?.title_opacity[0] > 0
    {
        picker.result.indeterminate = true;
        return Ok(picker.result);
    }
    // The frame planner supplies the painter's active scopes and clocks.
    let overlays = project
        .overlays
        .iter()
        .enumerate()
        .filter(|(_, overlay)| overlay.enabled && overlay.local_frame(frame).is_some());
    let shot_index = project
        .shots
        .iter()
        .position(|p| std::ptr::eq(p, shot))
        .context("active shot missing")?;
    let owners = std::iter::once(Some(InputOwner::Shot(shot_index)))
        .chain(overlays.map(|(index, _)| Some(InputOwner::Overlay(index))))
        .chain(std::iter::once(None));
    let scopes: Vec<_> = owners
        .zip(project.paint_scopes(shot, shot_clock, frame))
        .collect();
    for (owner, (layers, clock)) in scopes.into_iter().rev() {
        if picker.finished() {
            break;
        }
        let placement = match owner {
            Some(owner) => find_owner(&inspection.placements, owner),
            None => inspection.placement("V2:graphics@0"),
        };
        picker.scope(
            layers,
            placement,
            root_space(project, owner, clock),
            clock,
            area,
        )?;
    }
    if let PickArea::Point { at } = area
        && !front_only
        && picker.result.hits.is_empty()
        && !picker.result.indeterminate
        && shot.source.is_some()
        // When a drawable samples the plate, it owns where the footage appears.
        && !project.paint_scopes(shot, shot_clock, frame).any(|(layers, _)| uses_footage(layers))
    {
        let owner = InputOwner::Shot(shot_index);
        let [width, height] = project.size.map(f64::from);
        let local = root_space(project, Some(owner), shot_clock).to_local(*at);
        if contains([0., 0., width, height], local, 0.) {
            picker.result.fallback_placement = find_owner(&inspection.placements, owner)
                .filter(|placement| placement.enabled)
                .map(|placement| placement.path.clone());
        }
    }
    Ok(picker.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthoringDocument, Engine,
        graphics::{PreparedRasterAssets, PreparedTextLayouts},
    };
    use std::fs;

    #[test]
    fn marquee_does_not_pick_empty_corners_of_a_rotated_bounds() {
        let diamond = [[10., 0.], [20., 10.], [10., 20.], [0., 10.]];
        let overlap = |bounds| {
            has_area(&intersect_polygon(
                diamond.to_vec(),
                LayerSpace::default(),
                bounds,
            ))
        };
        assert!(overlap([8., 8., 11., 11.]));
        assert!(!overlap([1., 1., 4., 4.]));
    }

    #[test]
    fn svg_mask_reads_the_accepted_alpha_after_source_removal() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let sequences = fixture.path().join("sequences");
        fs::create_dir(&sequences)?;
        fs::write(
            fixture.path().join("project.proj"),
            "project mask fps 24/1 size 32x24\n[0,12) V1 shot\n",
        )?;
        let source = fixture.path().join("mask.svg");
        fs::write(
            &source,
            r##"<svg width="12" height="12"><path d="M0 0H6V12H0Z" fill="#ffffff"/></svg>"##,
        )?;
        fs::write(
            sequences.join("shot.json"),
            r#"{
                "background":{"color":[0,0,0]},
                "graphics":[
                    {"content":{"type":"rect","width":32,"height":24,"fill":[255,0,0,255]}},
                    {"content":{"type":"rect","width":32,"height":24,"fill":[0,255,0,255]},
                     "mask":{"position":[8,4],"content":{"type":"svg","source":"mask.svg"}}}
                ]
            }"#,
        )?;
        let (document, inputs) = crate::project_watch::ProjectInputs::load(fixture.path())?;
        let inspection = document.inspection()?;
        let project = document.project;
        let assets = PreparedRasterAssets::prepare(&project, None, &inputs)?;
        fs::remove_file(source)?;
        let text = PreparedTextLayouts::default();
        let assets = GeometryAssets {
            rasters: &assets,
            text: &text,
            canvas: project.size,
            native: &NativeCoverageCache::default(),
        };

        for (point, expected) in [([9.5, 6.5], vec![1, 0]), ([16.5, 6.5], vec![0])] {
            let hits = hit_test(&project, &inspection, assets, 3, point)?;
            assert!(!hits.indeterminate);
            assert_eq!(
                hits.hits
                    .into_iter()
                    .map(|hit| hit.layers[0])
                    .collect::<Vec<_>>(),
                expected,
            );
        }
        Ok(())
    }

    #[test]
    fn picks_masked_path_in_an_animated_group_on_each_occurrence_clock() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        fs::create_dir(fixture.path().join("sequences"))?;
        fs::write(
            fixture.path().join("project.proj"),
            "project geometry fps 24/1 size 64x48\n[0,12) V1 shot\n[12,24) V1 shot\n",
        )?;
        fs::write(
            fixture.path().join("sequences/shot.json"),
            r#"{
                "background":{"color":[0,0,0]},
                "graphics": [
                    {"content":{"type":"rect","width":64,"height":48,"fill":[255,0,0,255]}},
                    {"name":"Parent","position":[24,12],"span":[1,10],
                     "transform":{"rotation_degrees":90},
                     "motion":[{"frame":0,"x":0},{"frame":8,"x":16}],
                     "content":{"type":"transform_group","children":[
                         {"name":"Front","position":[6,8],
                          "content":{"type":"path","d":"M0 0H14V14H0Z","fill":[0,255,0,255]},
                          "mask":{"position":[2,0],"content":{"type":"path","d":"M0 0H5V14H0Z"}}}
                     ]}}
                ]
            }"#,
        )?;
        let document = AuthoringDocument::load(fixture.path(), None)?;
        let inspection = document.inspection()?;
        let project = document.project;
        let raster_assets = PreparedRasterAssets::default();
        let text = PreparedTextLayouts::default();
        let assets = GeometryAssets {
            rasters: &raster_assets,
            text: &text,
            canvas: project.size,
            native: &NativeCoverageCache::default(),
        };
        let mut engine = Engine::new(project.clone())?;

        for (frame, occurrence, move_x) in [
            (3, "V1:shot@0", 4.),
            (5, "V1:shot@0", 8.),
            (17, "V1:shot@12", 8.),
        ] {
            let front = LayerRef {
                occurrence: occurrence.into(),
                layers: vec![1, 0],
            };
            let back = LayerRef {
                occurrence: occurrence.into(),
                layers: vec![0],
            };
            let outline = resolve(&project, &inspection, assets, frame, &front)?;
            assert_eq!(outline.visibility, Visibility::Visible);
            assert_eq!(
                outline.local_to_composition,
                Some([[0., -1., 16. + move_x], [1., 0., 18.]]),
            );
            assert_eq!(outline.local_frame, Some(if frame == 3 { 2 } else { 4 }));
            assert_eq!(
                marquee_hits(
                    &project,
                    &inspection,
                    assets,
                    frame,
                    [10.5 + move_x, 21.5, 12.5 + move_x, 23.5]
                )?,
                vec![front.clone(), back.clone()],
            );
            assert_eq!(
                marquee_hits(
                    &project,
                    &inspection,
                    assets,
                    frame,
                    [10.5 + move_x, 28., 12.5 + move_x, 29.]
                )?,
                vec![back.clone()],
            );
            let picture = engine.render(frame)?;
            for (point, expected, color) in [
                (
                    [11.5 + move_x, 22.5],
                    vec![front, back.clone()],
                    [0, 255, 0],
                ),
                ([11.5 + move_x, 28.5], vec![back], [255, 0, 0]),
            ] {
                let hits = hit_test(&project, &inspection, assets, frame, point)?;
                assert_eq!(hits.hits, expected);
                assert!(!hits.indeterminate);
                let i = 4 * (point[1] as usize * 64 + point[0] as usize);
                assert_eq!(picture.rgba[i..i + 3], color);
            }
        }
        let inactive = resolve(
            &project,
            &inspection,
            assets,
            0,
            &LayerRef {
                occurrence: "V1:shot@0".into(),
                layers: vec![1, 0],
            },
        )?;
        assert_eq!(inactive.visibility, Visibility::Inactive);
        assert!(inactive.composition_quad.is_none());
        // A custom shader may synthesise or erase coverage. The lower layer
        // must not become the default pick while that coverage is unresolved.
        let mut filtered = project;
        let Content::TransformGroup { children } = &mut filtered.shots[0].graphics[1].content
        else {
            unreachable!("fixture contains a transform group");
        };
        children[0].filter = Some(crate::graphics::Filter {
            shader: "custom.wgsl".into(),
            amount: 100,
            mattes: Vec::new(),
        });
        let uncertain = hit_test(&filtered, &inspection, assets, 5, [19.5, 22.5])?;
        assert!(uncertain.indeterminate);
        assert!(uncertain.hits.is_empty());
        assert_eq!(uncertain.obstruction.unwrap().layers, vec![1, 0]);
        Ok(())
    }
}
