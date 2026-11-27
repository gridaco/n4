//! The authoring hierarchy behind one visual definition. This projection uses
//! validated graphics and the renderer's key sampler and needs no GPU or media.
use crate::camera::Camera;
use crate::graphics::{
    AnimatedScalar, BackdropFilter, Content, CornerRadius, CornerValues, Graphic, Keyframe,
    LabelText, MaskContent, MatteCoverage, MotionKey, PathData, RectGeometry, Scalar, ScalarKey,
    SourceEffect, Stroke, Timing,
};
use crate::render::channels::sample_scalar;
use serde::Serialize;
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Serialize)]
pub struct Track {
    pub name: String,
    /// Authored frames are local to the owning layer, even for a shared definition.
    pub keys: Vec<ScalarKey>,
    /// A type-on cue has zero visible graphemes before the first cue. Scalar
    /// animation instead holds its first key during that interval.
    #[serde(skip_serializing_if = "Option::is_none")]
    before_first: Option<f32>,
}

impl Track {
    /// Value at a local frame, with a fractional offset for drawing the curve.
    pub fn sample(&self, frame: u32, fraction: f64) -> f32 {
        if let Some(initial) = self.before_first
            && frame < self.keys[0].frame
        {
            return initial;
        }
        sample_scalar(&self.keys, frame, fraction)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Layer {
    pub name: String,
    pub kind: String,
    /// Address in the authored graphic tree. Inspector-only rows have no address.
    #[serde(rename = "authoredPath", skip_serializing_if = "Option::is_none")]
    pub authored_path: Option<Vec<usize>>,
    /// Definition-relative bounds. A missing end inherits the placement end.
    pub start: u32,
    pub end: Option<u32>,
    pub tracks: Vec<Track>,
    pub children: Vec<Layer>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub info: Vec<String>,
}

impl Layer {
    fn new(name: &str, kind: &str, start: u32, end: Option<u32>) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            authored_path: None,
            start,
            end,
            tracks: Vec::new(),
            children: Vec::new(),
            info: Vec::new(),
        }
    }

    fn child(&self, name: &str, kind: &str) -> Self {
        Self::new(name, kind, self.start, self.end)
    }

    fn keys(&mut self, name: &str, keys: &[ScalarKey]) {
        if !keys.is_empty() {
            self.tracks.push(Track {
                name: name.into(),
                keys: keys.to_vec(),
                before_first: None,
            });
        }
    }

    fn property(&mut self, name: &str, property: &AnimatedScalar<Scalar>) {
        if let AnimatedScalar::Keys(keys) = property {
            self.keys(name, keys);
        }
    }

    fn rectangle(&mut self, rect: &RectGeometry) {
        for (name, value) in [
            ("x", &rect.x),
            ("y", &rect.y),
            ("width", &rect.width),
            ("height", &rect.height),
        ] {
            self.property(name, value);
        }
        for (name, value) in [("rx", &rect.rx), ("ry", &rect.ry)] {
            if let Some(value) = value {
                self.property(name, value);
            }
        }
        if let Some(CornerRadius::Keys(keys)) = &rect.corner_radius {
            let uniform = keys
                .iter()
                .all(|k| matches!(k.value, CornerValues::Uniform(_)));
            for (i, name) in ["top-left", "top-right", "bottom-right", "bottom-left"]
                .iter()
                .enumerate()
            {
                let name = if uniform {
                    "corner-radius".into()
                } else {
                    format!("corner-radius {name}")
                };
                self.tracks.push(component_track(name, keys, |v| match v {
                    CornerValues::Uniform(value) => value,
                    CornerValues::Four(values) => values[i],
                }));
                if uniform {
                    break;
                }
            }
        }
    }

    fn stroke(&mut self, stroke: Option<&Stroke>) {
        if let Some(Stroke {
            width: AnimatedScalar::Keys(keys),
            ..
        }) = stroke
        {
            self.tracks
                .push(component_track("stroke-width".into(), keys, |width| {
                    finite(width.into())
                }));
        }
    }

    fn source_effect(&mut self, effect: &SourceEffect) {
        let child = match effect {
            SourceEffect::SourceBlur(blur) => {
                let mut child = self.child("Source blur", "effect");
                child.property("sigma", &blur.sigma);
                if let Some(amount) = &blur.amount {
                    child.property("amount", amount);
                }
                if let Some(band) = &blur.focus_band {
                    for (name, value) in [
                        ("center-y", &band.center_y),
                        ("half-width", &band.half_width),
                        ("feather", &band.feather),
                    ] {
                        child.property(name, value);
                    }
                }
                child
            }
            SourceEffect::RgbGrade(grade) => {
                let mut child = self.child("RGB grade", "effect");
                for (name, value) in [("gain", &grade.gain), ("offset", &grade.offset)] {
                    if let AnimatedScalar::Keys(keys) = value {
                        for (i, channel) in ["R", "G", "B"].iter().enumerate() {
                            child.tracks.push(component_track(
                                format!("{name} {channel}"),
                                keys,
                                |rgb| rgb[i],
                            ));
                        }
                    }
                }
                child
            }
        };
        self.children.push(child);
    }
}

fn component_track<T: Copy>(
    name: String,
    keys: &[Keyframe<T>],
    get: impl Fn(T) -> Scalar,
) -> Track {
    Track {
        name,
        keys: keys
            .iter()
            .map(|k| Keyframe {
                frame: k.frame,
                value: get(k.value),
                ease: k.ease,
            })
            .collect(),
        before_first: None,
    }
}

fn finite(value: f32) -> Scalar {
    value
        .try_into()
        .expect("validated graphics use finite values")
}

fn file_name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("Asset")
}

pub(super) fn layers(graphics: &[Graphic], cameras: &[Camera]) -> Vec<Layer> {
    cameras
        .iter()
        .map(camera)
        .chain(
            graphics
                .iter()
                .enumerate()
                .map(|(index, g)| graphic(g, 0, None, Some(vec![index]))),
        )
        .collect()
}

fn camera(camera: &Camera) -> Layer {
    let mut node = Layer::new(camera.id(), "camera", 0, None);
    match camera {
        Camera::Orthographic {
            center_x,
            center_y,
            zoom,
            roll,
            ..
        } => {
            node.info.push("Orthographic".into());
            for (name, value) in [
                ("center-x", center_x),
                ("center-y", center_y),
                ("zoom", zoom),
                ("roll", roll),
            ] {
                if let Some(value) = value {
                    node.property(name, value);
                }
            }
        }
        Camera::Perspective {
            position,
            rotation,
            fov_y,
            ..
        } => {
            node.info.push("Perspective".into());
            for (name, values) in [("position", position), ("rotation", rotation)] {
                for (axis, v) in ["x", "y", "z"].iter().zip(values) {
                    node.property(&format!("{name}-{axis}"), v);
                }
            }
            node.property("fov-y", fov_y);
        }
    }
    node
}

fn graphic(
    g: &Graphic,
    parent_start: u32,
    parent_end: Option<u32>,
    authored_path: Option<Vec<usize>>,
) -> Layer {
    let (start, end) = match g.span {
        Some([a, b]) => (parent_start + a, Some(parent_start + b)),
        None => (parent_start, parent_end),
    };
    let kind = match &g.content {
        Content::TransformGroup { .. } => "transform-group",
        Content::Group { .. } => "group",
        Content::Scene3d(_) => "scene3d",
        Content::Path(_) => "path",
        Content::Rect(_) => "rect",
        Content::Label { .. } => "text",
        Content::Footage { .. } => "footage",
        Content::Image { .. } => "image",
        Content::Svg { .. } => "svg",
        Content::Video(_) => "video",
        Content::Backdrop { .. } => "backdrop",
    };
    let mut node = Layer::new(g.name.as_deref().unwrap_or(kind), kind, start, end);
    transform_tracks(&mut node, g);
    match &g.content {
        Content::TransformGroup { children } | Content::Group { children } => {
            node.children
                .extend(children.iter().enumerate().map(|(index, child)| {
                    let child_path = authored_path.clone().map(|mut path| {
                        path.push(index);
                        path
                    });
                    graphic(child, start, end, child_path)
                }));
        }
        Content::Scene3d(scene) => spatial_children(&mut node, scene),
        Content::Rect(rect) => {
            node.rectangle(&rect.geometry);
            node.stroke(rect.stroke.as_ref());
        }
        Content::Path(path) => {
            for (name, value) in [
                ("stroke-start", &path.stroke_start),
                ("stroke-end", &path.stroke_end),
            ] {
                if let Some(v) = value {
                    node.property(name, v);
                }
            }
            node.stroke(path.stroke.as_ref());
            if let PathData::Animated(curve) = &path.d {
                let frames = curve
                    .keys
                    .iter()
                    .map(|k| k.frame.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                node.info
                    .push(format!("Path geometry poses at local frames {frames}"));
            }
        }
        Content::Label { text, .. } => label_details(&mut node, g, text),
        Content::Image { source } | Content::Svg { source } => {
            node.info.push(file_name(source).into())
        }
        Content::Video(video) => node.info.push(format!(
            "{} · source in {}",
            file_name(&video.source),
            video.source_in
        )),
        Content::Backdrop { filter } => {
            node.info.push(file_name(filter.shader()).into());
            if let BackdropFilter::Sampled2d { params, .. } = filter {
                for param in params {
                    match &param.value {
                        AnimatedScalar::Value(value) => {
                            node.info
                                .push(format!("{} = {}", param.name, f32::from(*value)))
                        }
                        AnimatedScalar::Keys(keys) => node.keys(&param.name, keys),
                    }
                }
            }
        }
        Content::Footage {
            history: Some(history),
        } => node.info.push(format!(
            "{} · {} prior source frames",
            file_name(&history.shader),
            history.frames
        )),
        Content::Footage { history: None } => {}
    }
    effect_children(&mut node, g);
    node.authored_path = authored_path;
    node
}

fn transform_tracks(node: &mut Layer, g: &Graphic) {
    if let Some(ch) = &g.channels {
        for (name, keyset) in [
            ("translate-x", &ch.x),
            ("translate-y", &ch.y),
            ("rotation", &ch.rotation),
            ("scale", &ch.scale),
            ("scale-x", &ch.scale_x),
            ("scale-y", &ch.scale_y),
            ("opacity", &ch.opacity),
        ] {
            if let Some(keys) = keyset {
                node.keys(name, keys);
            }
        }
    }
    if !g.motion.is_empty() {
        let pose_axes = [("x", 0.), ("y", 0.), ("scale", 1.), ("opacity", 1.)];
        let mut pose = node.child("Motion", "pose");
        for (axis, (name, default)) in pose_axes.into_iter().enumerate() {
            let get = |k: &MotionKey| [k.x, k.y, k.scale, k.opacity][axis];
            // A grouped pose supplies a shared key clock. Omit neutral axes,
            // which are also the defaults filled by deserialization.
            if g.motion.iter().any(|k| get(k) != default) {
                pose.tracks.push(Track {
                    name: name.into(),
                    keys: g
                        .motion
                        .iter()
                        .map(|k| ScalarKey {
                            frame: k.frame,
                            value: finite(get(k)),
                            ease: Timing::Linear,
                        })
                        .collect(),
                    before_first: None,
                });
            }
        }
        pose.info
            .push(format!("{} grouped pose keys", g.motion.len()));
        node.children.push(pose);
    }
}

fn spatial_children(node: &mut Layer, scene: &crate::graphics::Scene3d) {
    for mesh in &scene.models {
        let mut model = node.child(mesh.id.as_deref().unwrap_or(&mesh.prim), "usd");
        model
            .info
            .push(format!("{} · {}", file_name(&mesh.source), mesh.prim));
        for (name, value) in [
            ("rotation-x", &mesh.rotation_x),
            ("rotation-y", &mesh.rotation_y),
            ("rotation-z", &mesh.rotation_z),
        ] {
            model.property(name, value);
        }
        for part in &mesh.parts {
            let mut child = model.child(&part.prim, "rigid-part");
            for (name, channel) in [
                ("rotation-x", &part.rotation_x),
                ("rotation-y", &part.rotation_y),
                ("rotation-z", &part.rotation_z),
            ] {
                child.property(name, channel);
            }
            model.children.push(child);
        }
        for surface in &mesh.surfaces {
            let mut child = model.child(&surface.slot, "material-surface");
            child.info.push(format!(
                "{:?} · {} × {}",
                surface.role, surface.size[0], surface.size[1]
            ));
            child.children.extend(
                surface
                    .layers
                    .iter()
                    .map(|layer| graphic(layer, node.start, node.end, None)),
            );
            model.children.push(child);
        }
        node.children.push(model);
    }
    if let Some(look) = &scene.studio {
        let mut child = node.child("Studio lighting", "studio");
        child.property("environment-rotation", &look.environment_rotation);
        child.property("environment-intensity", &look.environment_intensity);
        child.property("exposure", &look.exposure);
        node.children.push(child);
    }
    for plane in &scene.planes {
        let mut child = node.child(plane.id.as_deref().unwrap_or("Text plane"), "plane");
        child
            .children
            .push(graphic(&plane.text, node.start, node.end, None));
        node.children.push(child);
    }
    for light in &scene.lights {
        let mut child = node.child("Distant light", "distant-light");
        child
            .info
            .push(format!("Intensity {}", f32::from(light.intensity)));
        node.children.push(child);
    }
}

fn label_details(node: &mut Layer, g: &Graphic, text: &LabelText) {
    let copy = match text {
        LabelText::Spans(spans) => spans.iter().map(|s| s.text.as_str()).collect::<String>(),
        LabelText::Plain(copy) => copy.clone(),
    };
    if let Some(cues) = &g.type_on {
        let all = copy.graphemes(true).count();
        node.tracks.push(Track {
            name: "type-on".into(),
            keys: cues
                .iter()
                .map(|&(frame, count)| ScalarKey {
                    frame,
                    value: finite(count.map_or(all, |n| n as usize).min(all) as f32),
                    ease: Timing::Hold,
                })
                .collect(),
            before_first: Some(0.),
        });
    }
    node.info.push(copy);
    for (i, animator) in g.text_animators.iter().enumerate() {
        let mut child = node.child(&format!("Text animator {}", i + 1), "text-animator");
        child.info.push(format!(
            "Target {}",
            animator.target.as_deref().unwrap_or("all text")
        ));
        if let Some([r, g, b, a]) = animator.fill {
            child
                .info
                .push(format!("Fill #{r:02x}{g:02x}{b:02x}{a:02x}"));
        }
        if let Some([x, y]) = animator.offset {
            child.info.push(format!("Offset {x}, {y}"));
        }
        for (name, keys) in [("start", &animator.start), ("end", &animator.end)] {
            if let Some(keys) = keys {
                child.keys(name, keys);
            }
        }
        node.children.push(child);
    }
}

fn effect_children(node: &mut Layer, g: &Graphic) {
    if let Some(mask) = &g.mask {
        let mut child = node.child("Mask", "mask");
        match &mask.content {
            MaskContent::Rect(geometry) => child.rectangle(geometry),
            MaskContent::Path { .. } => child.info.push("Path".into()),
            MaskContent::Svg { source } => child.info.push(file_name(source).into()),
        }
        if let Some(shader) = &mask.shader {
            child.info.push(format!(
                "{} · period {} frames",
                file_name(&shader.shader),
                shader.period_frames
            ));
        }
        node.children.push(child);
    }
    for effect in &g.effects {
        node.source_effect(effect);
    }
    if let Some(filter) = &g.filter {
        let mut child = node.child(file_name(&filter.shader), "filter");
        for matte in &filter.mattes {
            let query = if let Some(owner) = &matte.occluded_by {
                format!("{} occluded by {owner}", matte.object)
            } else {
                let coverage = match matte.coverage {
                    MatteCoverage::Projected => "projected",
                    MatteCoverage::Visible => "visible",
                };
                format!("{} {coverage}", matte.object)
            };
            child.info.push(format!("{} = {query}", matte.name));
        }
        node.children.push(child);
    }
    if let Some(underlay) = &g.underlay {
        let mut child = node.child(file_name(&underlay.shader), "underlay");
        child
            .info
            .push(format!("Sigma {} px", underlay.sigma_px.get()));
        node.children.push(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use serde_json::json;

    #[test]
    fn group_offsets_accumulate_once_and_scalar_samples_follow_the_renderer() -> Result<()> {
        let parent: Graphic = serde_json::from_value(json!({
            "name": "Phone", "span": [30,70],
            "channels": {"y":[{"frame":0,"value":8},{"frame":20,"value":0}]},
            "content":{"type":"transform_group","children":[{
                "name": "Card", "span":[5,25],
                "channels":{"opacity":[{"frame":1,"value":0,"ease":{"cubic":[0.2,0.8,0.3,1]}},{"frame":9,"value":1}]},
                "content":{"type":"group","children":[{"name":"Fill","content":{"type":"path","d":"M0 0H10V10Z"}}]}
            }]}
        }))?;
        let tree = graphic(&parent, 0, None, Some(vec![2]));
        let card = &tree.children[0];
        let fill = &card.children[0];
        assert_eq!(card.authored_path.as_deref(), Some([2, 0].as_slice()));
        assert_eq!(fill.authored_path.as_deref(), Some([2, 0, 0].as_slice()));
        assert_eq!((card.start, card.end), (35, Some(55)));
        assert_eq!((fill.start, fill.end), (35, Some(55)));
        assert!(fill.tracks.is_empty());
        assert_eq!(card.tracks.len(), 1);
        let Content::TransformGroup { children } = &parent.content else {
            panic!("expected group")
        };
        for (frame, fraction) in [(0, 0.), (1, 0.), (2, 0.75), (8, 0.), (9, 0.)] {
            let expected = children[0].motion_at(frame, fraction).opacity;
            assert_eq!(card.tracks[0].sample(frame, fraction), expected);
        }
        Ok(())
    }

    #[test]
    fn type_on_reports_discrete_grapheme_counts_and_no_synthetic_key() -> Result<()> {
        let layer: Graphic = serde_json::from_value(json!({
            "type_on":[[2,1],[8,null]],
            "content":{"type":"label","text":"e\u{301}x","font":"unused.ttf","size":10,"width":100,"rgb":[0,0,0]}
        }))?;
        let view = graphic(&layer, 0, None, None);
        let reveal = &view.tracks[0];
        assert_eq!(reveal.keys.len(), 2);
        assert_eq!(reveal.sample(1, 0.), 0.);
        assert_eq!(reveal.sample(2, 0.), 1.);
        assert_eq!(reveal.sample(7, 0.999), 1.);
        assert_eq!(reveal.sample(8, 0.), 2.);
        Ok(())
    }

    #[test]
    fn lens_mesh_and_matte_bindings_have_separate_authored_owners() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        std::fs::create_dir(root.join("sequences"))?;
        std::fs::write(
            root.join("project.proj"),
            "project art fps 24/1 size 128x128\n[0,60) V1 graphic\n",
        )?;
        std::fs::write(
            root.join("sequences/graphic.xml"),
            r##"<sequence camera="close" background="#ffffff">
          <camera id="close" projection="perspective" position="0 0 3">
            <animate attributeName="fov-y"><key frame="44" value="11.6"/></animate>
          </camera>
          <scene3d pixels-per-meter="20">
            <usd id="card" source="card.usdz" prim="/Card">
              <animate attributeName="rotation-y"><key frame="44" value="60"/></animate>
            </usd>
            <plane id="title" position="0 0 0"><text font="title.ttf" size="16" width="80" fill="#ffffff"><copy>AB</copy></text></plane>
            <filter source="ink.wgsl"><matte name="hidden" object="title" occluded-by="card"/></filter>
          </scene3d>
        </sequence>"##,
        )?;
        let project = crate::AuthoringDocument::load(root, None)?.inspection()?;
        let rows = &project.definitions["graphic"].layers;
        let lens = rows.iter().find(|row| row.name == "close").unwrap();
        let scene = rows.iter().find(|row| row.kind == "scene3d").unwrap();
        assert_eq!(lens.authored_path, None);
        assert!(scene.authored_path.is_some());
        let card = scene
            .children
            .iter()
            .find(|row| row.name == "card")
            .unwrap();
        let filter = scene
            .children
            .iter()
            .find(|row| row.kind == "filter")
            .unwrap();
        assert_eq!(lens.tracks[0].name, "fov-y");
        assert_eq!(lens.tracks[0].sample(44, 0.), 11.6);
        assert_eq!(card.tracks[0].name, "rotation-y");
        assert_eq!(card.tracks[0].sample(44, 0.), 60.);
        assert!(filter.tracks.is_empty());
        assert!(
            filter
                .info
                .iter()
                .any(|binding| binding == "hidden = title occluded by card")
        );
        Ok(())
    }
}
