//! Decode the editable XML artwork vocabulary to the native detail document.
//! The master resolves assets, placement clocks and the typed graphics model.
mod spatial;
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

type Object = Map<String, Value>;
type Element<'a, 'i> = Node<'a, 'i>;

pub(crate) fn decode(bytes: &[u8]) -> Result<Value> {
    let doc = Document::parse(std::str::from_utf8(bytes).context("sequence XML must be UTF-8")?)?;
    let root = doc.root_element();
    located(root, || {
        tag(root, "sequence")?;
        let mut attrs = Attrs::new(root)?;
        let mut detail = Object::new();
        attrs.read(
            &mut detail,
            &[
                ("name", "name", Kind::String),
                ("camera", "camera", Kind::String),
                ("source", "source", Kind::String),
                ("source-in", "source_in", Kind::Number),
            ],
        )?;
        if let Some(color) = attrs.take("background") {
            detail.insert(
                "background".into(),
                json!({"color":color_bytes(color, false)?}),
            );
        }
        attrs.finish()?;
        let mut graphics = Vec::new();
        let mut cameras = Vec::new();
        for child in children(root)? {
            if child.has_tag_name("camera") {
                cameras.push(located(child, || read_camera(child))?);
            } else {
                graphics.push(graphic(child)?);
            }
        }
        if !cameras.is_empty() {
            detail.insert("cameras".into(), cameras.into());
        }
        detail.insert("graphics".into(), Value::Array(graphics));
        Ok(Value::Object(detail))
    })
}

fn located<T>(node: Element, f: impl FnOnce() -> Result<T>) -> Result<T> {
    f().with_context(|| {
        let pos = node.document().text_pos_at(node.range().start);
        format!(
            "line {}, column {}, <{}>",
            pos.row,
            pos.col,
            node.tag_name().name()
        )
    })
}

fn tag(node: Element, wanted: &str) -> Result<()> {
    ensure!(node.tag_name().name() == wanted, "expected <{wanted}>");
    Ok(())
}

fn children<'a, 'i>(node: Element<'a, 'i>) -> Result<Vec<Element<'a, 'i>>> {
    for text in node.children().filter(Node::is_text) {
        ensure!(
            text.text().unwrap_or("").trim().is_empty(),
            "unexpected literal text"
        );
    }
    Ok(node.children().filter(Node::is_element).collect())
}

struct Attrs<'a>(BTreeMap<&'a str, &'a str>);

impl<'a> Attrs<'a> {
    fn new(node: Element<'a, '_>) -> Result<Self> {
        ensure!(
            node.tag_name().namespace().is_none(),
            "unexpected XML namespace"
        );
        let mut attrs = BTreeMap::new();
        for attr in node.attributes() {
            ensure!(
                attr.namespace().is_none(),
                "unexpected namespaced attribute {}",
                attr.name()
            );
            attrs.insert(attr.name(), attr.value());
        }
        Ok(Self(attrs))
    }

    fn take(&mut self, name: &str) -> Option<&'a str> {
        self.0.remove(name)
    }

    fn need(&mut self, name: &str) -> Result<&'a str> {
        self.take(name)
            .with_context(|| format!("missing attribute {name}"))
    }

    fn read(&mut self, obj: &mut Object, fields: &[(&str, &str, Kind)]) -> Result<()> {
        for &(attr, field, kind) in fields {
            if let Some(value) = self.take(attr) {
                let v = kind
                    .parse(value)
                    .with_context(|| format!("invalid {attr}={value:?}"))?;
                put(obj, field, v)?;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<()> {
        if let Some((name, _)) = self.0.first_key_value() {
            bail!("unknown attribute {name}");
        }
        Ok(())
    }
}

fn put(obj: &mut Object, name: &str, v: Value) -> Result<()> {
    ensure!(!obj.contains_key(name), "{name} has multiple definitions");
    obj.insert(name.to_owned(), v);
    Ok(())
}

#[derive(Clone, Copy)]
enum Kind {
    String,
    Number,
    Fraction,
    Pair,
    Triple,
    RgbCoefficients,
    Radius,
    Color,
    Rgb,
    Fill,
    Token,
}

fn number(s: &str) -> Result<Value> {
    if let Ok(n) = s.parse::<i64>() {
        return Ok(Value::from(n));
    }
    let f: f64 = s.parse()?;
    ensure!(f.is_finite(), "number must be finite");
    Ok(Value::from(f))
}

fn numbers(s: &str) -> Result<Vec<Value>> {
    s.split_ascii_whitespace().map(number).collect()
}

fn color_bytes(s: &str, alpha: bool) -> Result<Vec<u8>> {
    let hex = s.strip_prefix('#').context("expected hexadecimal color")?;
    ensure!(
        hex.is_ascii() && (hex.len() == 6 || alpha && hex.len() == 8),
        "invalid color {s:?}"
    );
    let mut output = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()?;
    if alpha && output.len() == 3 {
        output.push(255);
    }
    Ok(output)
}

impl Kind {
    fn parse(self, s: &str) -> Result<Value> {
        Ok(match self {
            Self::String => Value::from(s),
            Self::Token => {
                ensure!(!s.contains('_'), "use hyphens in enum names");
                Value::from(s.replace('-', "_"))
            }
            Self::Number => number(s)?,
            Self::Fraction => {
                let value = number(s)?;
                ensure!(
                    value.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)),
                    "fraction must be between 0 and 1"
                );
                value
            }
            Self::Pair => {
                let v = numbers(s)?;
                ensure!(v.len() == 2, "expected a pair of coordinates");
                Value::Array(v)
            }
            Self::RgbCoefficients => {
                let v = numbers(s)?;
                ensure!(v.len() == 3, "expected three RGB coefficients");
                Value::Array(v)
            }
            Self::Triple => {
                let v = numbers(s)?;
                ensure!(v.len() == 3, "expected three components");
                Value::Array(v)
            }
            Self::Radius => {
                let mut v = numbers(s)?;
                ensure!(
                    v.iter()
                        .all(|value| value.as_f64().is_some_and(|n| n >= 0.)),
                    "corner radius must be nonnegative"
                );
                match v.len() {
                    1 => v.remove(0),
                    4 => Value::Array(v),
                    _ => bail!("corner-radius requires one or four lengths"),
                }
            }
            Self::Rgb => json!(color_bytes(s, false)?),
            Self::Fill if s == "none" => Value::Null,
            Self::Fill | Self::Color => json!(color_bytes(s, true)?),
        })
    }
}

use Kind::{Color, Fill, Fraction, Number, Pair, Radius, Rgb, String as Str, Token, Triple};

fn shape_style(attrs: &mut Attrs, content: &mut Object) -> Result<()> {
    attrs.read(content, &[("fill", "fill", Fill)])?;
    let mut stroke = Object::new();
    attrs.read(
        &mut stroke,
        &[
            ("stroke", "rgba", Color),
            ("stroke-width", "width", Number),
            ("stroke-linecap", "cap", Token),
            ("stroke-linejoin", "join", Token),
        ],
    )?;
    if !stroke.is_empty() {
        content.insert("stroke".into(), stroke.into());
    }
    Ok(())
}

fn read_camera(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut camera = Object::new();
    camera.insert("id".into(), attrs.need("id")?.into());
    let projection = attrs.take("projection").unwrap_or("orthographic");
    camera.insert("projection".into(), projection.into());
    let properties: &[&str] = match projection {
        "orthographic" => &["center-x", "center-y", "zoom", "roll"],
        "perspective" => &[
            "fov-y",
            "position-x",
            "position-y",
            "position-z",
            "rotation-x",
            "rotation-y",
            "rotation-z",
        ],
        _ => bail!("unknown camera projection {projection:?}"),
    };
    for &name in properties {
        if let Some(value) = attrs.take(name) {
            put(&mut camera, &name.replace('-', "_"), number(value)?)?;
        }
    }
    let mut vectors = Vec::new();
    if projection == "perspective" {
        for key in ["position", "rotation"] {
            if let Some(value) = attrs.take(key) {
                vectors.push((key, Triple.parse(value)?));
            }
        }
    }
    attrs.finish()?;
    for child in children(node)? {
        tag(child, "animate")?;
        let mut a = Attrs::new(child)?;
        let name = a.need("attributeName")?;
        ensure!(
            properties.contains(&name),
            "unknown {projection} camera property {name}"
        );
        let keys = scalar_track(child, a, Number)?;
        put(&mut camera, &name.replace('-', "_"), keys)?;
    }
    if projection == "perspective" {
        for key in ["position", "rotation"] {
            let value = vectors
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, v)| v);
            let mut axes = Vec::with_capacity(3);
            for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                let scalar = camera.remove(&format!("{key}_{axis}"));
                ensure!(
                    value.is_none() || scalar.is_none(),
                    "{key} and {key}-{axis} both define the same camera property"
                );
                axes.push(
                    scalar
                        .or_else(|| value.map(|v| v[i].clone()))
                        .unwrap_or_else(|| 0.into()),
                );
            }
            camera.insert(key.into(), axes.into());
        }
    }
    Ok(camera.into())
}

fn graphic(node: Element) -> Result<Value> {
    located(node, || read_graphic(node))
}

fn read_graphic(node: Element) -> Result<Value> {
    // Like the JSON document reader, bound recursive descent on user-authored input.
    ensure!(
        node.ancestors().take(128).count() < 128,
        "graphic nesting exceeds document depth limit"
    );
    let kind = match node.tag_name().name() {
        "text" => "label",
        "g" => "group",
        "transform-group" => "transform_group",
        "scene3d" => "scene3d",
        name @ ("rect" | "path" | "image" | "svg" | "video" | "footage" | "backdrop") => name,
        other => bail!("unknown graphic <{other}>"),
    };
    let mut attrs = Attrs::new(node)?;
    let mut layer = Object::new();
    attrs.read(
        &mut layer,
        &[
            ("name", "name", Str),
            ("position", "position", Pair),
            ("span", "span", Pair),
            ("blend", "blend", Token),
        ],
    )?;
    let mut transform = Object::new();
    attrs.read(
        &mut transform,
        &[
            ("pivot", "pivot", Pair),
            ("rotation", "rotation_degrees", Number),
            ("scale", "scale", Number),
            ("scale-x", "scale_x", Number),
            ("scale-y", "scale_y", Number),
        ],
    )?;
    if !transform.is_empty() {
        layer.insert("transform".into(), transform.into());
    }
    let mut content = Object::from_iter([("type".into(), Value::from(kind))]);
    match kind {
        "scene3d" => attrs.read(
            &mut content,
            &[("pixels-per-meter", "pixels_per_meter", Number)],
        )?,
        "image" | "svg" => attrs.read(&mut content, &[("source", "source", Str)])?,
        "video" => attrs.read(
            &mut content,
            &[
                ("source", "source", Str),
                ("source-in", "source_in", Number),
                ("start", "start", Number),
                ("duration", "duration", Number),
                ("size", "size", Pair),
            ],
        )?,
        "backdrop" => {
            let mut filter =
                Object::from_iter([("shader".into(), Value::from(attrs.need("source")?))]);
            attrs.read(
                &mut filter,
                &[
                    ("sample-radius", "sample_radius_px", Number),
                    ("sigma", "sigma_px", Number),
                ],
            )?;
            ensure!(
                filter.len() == 2,
                "backdrop requires exactly one of sample-radius or sigma"
            );
            content.insert("filter".into(), filter.into());
        }
        "rect" => {
            attrs.read(
                &mut content,
                &[
                    ("x", "x", Number),
                    ("y", "y", Number),
                    ("width", "width", Number),
                    ("height", "height", Number),
                    ("corner-radius", "corner_radius", Radius),
                ],
            )?;
            shape_style(&mut attrs, &mut content)?;
        }
        "path" => {
            attrs.read(
                &mut content,
                &[
                    ("d", "d", Str),
                    ("stroke-start", "stroke_start", Fraction),
                    ("stroke-end", "stroke_end", Fraction),
                ],
            )?;
            if let Some(rule) = attrs.take("fill-rule") {
                let rule = match rule {
                    "evenodd" => "even_odd",
                    "nonzero" => "nonzero",
                    _ => bail!("unknown fill-rule {rule}"),
                };
                content.insert("fill_rule".into(), Value::from(rule));
            }
            shape_style(&mut attrs, &mut content)?;
        }
        "label" => attrs.read(
            &mut content,
            &[
                ("font", "font", Str),
                ("width", "width", Number),
                ("size", "size", Number),
                ("fill", "rgb", Rgb),
            ],
        )?,
        _ => {}
    }
    attrs.finish()?;
    let mut animators = Vec::new();
    let mut effects = Vec::new();
    let mut nested = Vec::new();
    let mut params = Vec::new();
    let mut channels = Object::new();
    for child in children(node)? {
        located(child, || {
            let name = child.tag_name().name();
            match name {
                "param" if kind == "backdrop" => {
                    ensure!(
                        node.has_attribute("sample-radius"),
                        "parameters require a 2D backdrop"
                    );
                    let mut attrs = Attrs::new(child)?;
                    let name = attrs.need("name")?;
                    let value = if let Some(raw) = attrs.take("value") {
                        attrs.finish()?;
                        ensure!(
                            children(child)?.is_empty(),
                            "choose a parameter value or keyframes"
                        );
                        number(raw)?
                    } else {
                        scalar_track(child, attrs, Number)?
                    };
                    params.push(json!({"name":name, "value":value}));
                }
                "history" if kind == "footage" => {
                    let mut attrs = Attrs::new(child)?;
                    let shader = attrs.need("source")?;
                    let frames = number(attrs.need("frames")?)?;
                    attrs.finish()?;
                    ensure!(children(child)?.is_empty(), "history must be empty");
                    put(
                        &mut content,
                        "history",
                        json!({ "shader": shader, "frames": frames }),
                    )?;
                }
                "animate" => {
                    let mut attrs = Attrs::new(child)?;
                    let property = attrs.need("attributeName")?;
                    match (kind, property) {
                        ("rect", "x" | "y" | "width" | "height" | "corner-radius") => {
                            let radius = property == "corner-radius";
                            let keys =
                                scalar_track(child, attrs, if radius { Radius } else { Number })?;
                            put(
                                &mut content,
                                if radius { "corner_radius" } else { property },
                                keys,
                            )?;
                        }
                        ("path" | "rect", "stroke-width") => {
                            let stroke = content
                                .get_mut("stroke")
                                .and_then(Value::as_object_mut)
                                .context("stroke-width animation requires a stroke")?;
                            put(stroke, "width", scalar_track(child, attrs, Number)?)?;
                        }
                        ("path", "d") => put(&mut content, "d", curve(child, attrs)?)?,
                        ("path", "stroke-start" | "stroke-end") => put(
                            &mut content,
                            &property.replace('-', "_"),
                            scalar_track(child, attrs, Fraction)?,
                        )?,
                        (
                            _,
                            "translate-x" | "translate-y" | "opacity" | "rotation" | "scale"
                            | "scale-x" | "scale-y",
                        ) => {
                            ensure!(
                                !node.has_attribute(property),
                                "{property} has multiple definitions"
                            );
                            let field = property
                                .strip_prefix("translate-")
                                .unwrap_or(property)
                                .replace('-', "_");
                            put(&mut channels, &field, scalar_track(child, attrs, Number)?)?;
                        }
                        _ => bail!(
                            "unknown animation property {property:?} on <{}>",
                            node.tag_name().name()
                        ),
                    }
                }
                "filter" => put(
                    &mut layer,
                    "filter",
                    spatial::filter(child, kind == "scene3d")?,
                )?,
                "studio" if kind == "scene3d" => {
                    put(&mut content, "studio", spatial::studio(child)?)?
                }
                "distant-light" | "usd" | "plane" if kind == "scene3d" => {
                    let (field, value) = match name {
                        "distant-light" => ("lights", spatial::light(child)?),
                        "usd" => ("models", spatial::instance(child)?),
                        _ => ("planes", spatial::plane(child)?),
                    };
                    content
                        .entry(field)
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .unwrap()
                        .push(value);
                }
                "mask" => put(&mut layer, "mask", mask(child)?)?,
                "source-blur" | "rgb-grade" => effects.push(effect_parameters(child)?),
                "motion" => {
                    let static_scale = ["scale", "scale-x", "scale-y"]
                        .iter()
                        .any(|&attr| node.has_attribute(attr));
                    put(&mut layer, "motion", motion(child, static_scale)?)?;
                }
                "type-on" if kind == "label" => put(&mut layer, "type_on", type_on(child)?)?,
                "copy" if kind == "label" => put(&mut content, "text", copy(child)?)?,
                "text-animator" if kind == "label" => animators.push(text_animator(child)?),
                "underline" if kind == "label" => put(
                    &mut content,
                    "underline",
                    simple(
                        child,
                        &[
                            ("style", "style", Token),
                            ("fill", "rgba", Color),
                            ("diameter", "diameter", Number),
                            ("offset", "offset", Number),
                            ("spacing", "spacing", Number),
                        ],
                    )?,
                )?,
                "text-anchor" => put(
                    &mut layer,
                    "text_anchor",
                    simple(
                        child,
                        &[("target", "target", Str), ("point", "point", Token)],
                    )?,
                )?,
                _ if kind == "group" || kind == "transform_group" => {
                    nested.push(read_graphic(child)?)
                }
                _ => bail!("unexpected <{name}> child of <{}>", node.tag_name().name()),
            }
            Ok(())
        })?;
    }
    if content.contains_key("stroke_start") || content.contains_key("stroke_end") {
        ensure!(
            node.has_attribute("stroke"),
            "stroke trim requires a stroke"
        );
    }
    if kind == "group" || kind == "transform_group" {
        content.insert("children".into(), nested.into());
    }
    if !params.is_empty() {
        content["filter"]["params"] = params.into();
    }
    if !animators.is_empty() {
        layer.insert("text_animators".into(), animators.into());
    }
    if !effects.is_empty() {
        layer.insert("effects".into(), effects.into());
    }
    if !channels.is_empty() {
        layer.insert("channels".into(), channels.into());
    }
    layer.insert("content".into(), content.into());
    Ok(layer.into())
}

fn simple(node: Element, fields: &[(&str, &str, Kind)]) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut obj = Object::new();
    attrs.read(&mut obj, fields)?;
    attrs.finish()?;
    ensure!(children(node)?.is_empty(), "expected no child elements");
    Ok(obj.into())
}

/// The layer owns effect order. Each effect owns its parameters and local keyframes.
fn effect_parameters(node: Element) -> Result<Value> {
    let name = node.tag_name().name();
    let fields: &[(&str, &str, Kind)] = match name {
        "source-blur" => &[("sigma", "sigma", Number), ("amount", "amount", Fraction)],
        "rgb-grade" => &[
            ("gain", "gain", Kind::RgbCoefficients),
            ("offset", "offset", Kind::RgbCoefficients),
        ],
        "focus-band" => &[
            ("center-y", "center_y", Number),
            ("half-width", "half_width", Number),
            ("feather", "feather", Number),
        ],
        _ => bail!("unknown source effect or coverage <{name}>"),
    };
    let mut obj = Object::new();
    if name != "focus-band" {
        obj.insert("type".into(), name.replace('-', "_").into());
    }
    let mut attrs = Attrs::new(node)?;
    attrs.read(&mut obj, fields)?;
    attrs.finish()?;
    for child in children(node)? {
        located(child, || {
            if name == "source-blur" && child.tag_name().name() == "focus-band" {
                return put(&mut obj, "focus_band", effect_parameters(child)?);
            }
            tag(child, "animate")?;
            let mut attrs = Attrs::new(child)?;
            let property = attrs.need("attributeName")?;
            let &(_, field, kind) = fields
                .iter()
                .find(|(name, _, _)| *name == property)
                .with_context(|| {
                    format!(
                        "unknown animation property {property:?} on <{}>",
                        node.tag_name().name()
                    )
                })?;
            put(&mut obj, field, scalar_track(child, attrs, kind)?)
        })?;
    }
    Ok(obj.into())
}

fn mask(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut mask = Object::new();
    attrs.read(&mut mask, &[("position", "position", Pair)])?;
    attrs.finish()?;
    let nodes = children(node)?;
    let [shape] = nodes.as_slice() else {
        bail!("mask requires one shape")
    };
    ensure!(
        matches!(shape.tag_name().name(), "rect" | "path" | "svg"),
        "unsupported mask shape"
    );
    let Value::Object(mut layer) = graphic(*shape)? else {
        unreachable!()
    };
    let content = layer.remove("content").expect("graphic has content");
    ensure!(
        layer.is_empty(),
        "put mask position on the mask element; mask shape only defines coverage"
    );
    ensure!(
        content.get("stroke_start").is_none() && content.get("stroke_end").is_none(),
        "path masks use filled coverage and cannot trim a stroke"
    );
    mask.insert("content".into(), content);
    Ok(mask.into())
}

fn copy(node: Element) -> Result<Value> {
    Attrs::new(node)?.finish()?;
    if !node.children().any(|n| n.is_element()) {
        return Ok(node
            .children()
            .filter(Node::is_text)
            .filter_map(|n| n.text())
            .collect::<String>()
            .into());
    }
    let mut spans = Vec::new();
    for child in node.children() {
        if child.is_text() {
            spans.push(json!({"text":child.text().unwrap_or("")}));
        }
        if child.is_element() {
            spans.push(located(child, || {
                tag(child, "span")?;
                let mut attrs = Attrs::new(child)?;
                let mut span = Object::new();
                attrs.read(&mut span, &[("id", "id", Str)])?;
                attrs.finish()?;
                ensure!(
                    !child.children().any(|n| n.is_element()),
                    "span only accepts literal text"
                );
                span.insert(
                    "text".into(),
                    child
                        .children()
                        .filter(Node::is_text)
                        .filter_map(|n| n.text())
                        .collect::<String>()
                        .into(),
                );
                Ok(Value::Object(span))
            })?);
        }
    }
    Ok(spans.into())
}

fn keys(node: Element, mut f: impl FnMut(&mut Attrs) -> Result<Value>) -> Result<Value> {
    let mut result = Vec::new();
    let mut previous = None;
    for key in children(node)? {
        result.push(located(key, || {
            tag(key, "key")?;
            ensure!(children(key)?.is_empty(), "key cannot have children");
            let mut attrs = Attrs::new(key)?;
            let frame: u32 = attrs.0.get("frame").context("missing frame")?.parse()?;
            ensure!(
                previous.is_none_or(|p| frame > p),
                "key frames must strictly increase"
            );
            previous = Some(frame);
            let v = f(&mut attrs)?;
            attrs.finish()?;
            Ok(v)
        })?);
    }
    ensure!(!result.is_empty(), "track requires at least one key");
    Ok(result.into())
}

fn timing(s: &str) -> Result<Value> {
    if let Some(inner) = s.strip_prefix("cubic(").and_then(|s| s.strip_suffix(')')) {
        let v = numbers(inner)?;
        ensure!(v.len() == 4, "cubic timing requires four controls");
        Ok(json!({"cubic":v}))
    } else {
        ensure!(matches!(s, "linear" | "hold"), "unknown scalar easing {s}");
        Ok(s.into())
    }
}

fn scalar_track(node: Element, attrs: Attrs, value_kind: Kind) -> Result<Value> {
    attrs.finish()?;
    keys(node, |attrs| {
        let mut key = Object::new();
        key.insert("frame".into(), number(attrs.need("frame")?)?);
        key.insert("value".into(), value_kind.parse(attrs.need("value")?)?);
        if let Some(ease) = attrs.take("ease") {
            key.insert("ease".into(), timing(ease)?);
        }
        Ok(key.into())
    })
}

fn curve(node: Element, mut attrs: Attrs) -> Result<Value> {
    let closed: bool = attrs.need("closed")?.parse()?;
    attrs.finish()?;
    let keys = keys(node, |attrs| {
        let mut key = Object::new();
        attrs.read(
            &mut key,
            &[("frame", "frame", Number), ("ease", "ease", Token)],
        )?;
        let mut points = Vec::new();
        for point in attrs.need("points")?.split_ascii_whitespace() {
            let (x, y) = point
                .split_once(',')
                .context("expected x,y pairs in points")?;
            points.push(json!([number(x)?, number(y)?]));
        }
        key.insert("points".into(), points.into());
        Ok(key.into())
    })?;
    Ok(json!({"closed":closed,"keys":keys}))
}

fn motion(node: Element, static_scale: bool) -> Result<Value> {
    Attrs::new(node)?.finish()?;
    keys(node, |attrs| {
        let mut key = Object::new();
        attrs.read(
            &mut key,
            &[
                ("frame", "frame", Number),
                ("x", "x", Number),
                ("y", "y", Number),
                ("scale", "scale", Number),
                ("opacity", "opacity", Number),
            ],
        )?;
        ensure!(
            !(static_scale && key.contains_key("scale")),
            "scale has multiple definitions on the layer and motion key"
        );
        Ok(key.into())
    })
}

fn type_on(node: Element) -> Result<Value> {
    Attrs::new(node)?.finish()?;
    keys(node, |attrs| {
        let frame = number(attrs.need("frame")?)?;
        let value = attrs.need("value")?;
        Ok(json!([
            frame,
            if value == "all" {
                Value::Null
            } else {
                number(value)?
            }
        ]))
    })
}

fn text_animator(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut obj = Object::new();
    attrs.read(
        &mut obj,
        &[
            ("target", "target", Str),
            ("fill", "fill", Color),
            ("offset", "offset", Pair),
        ],
    )?;
    attrs.finish()?;
    for child in children(node)? {
        located(child, || {
            tag(child, "animate")?;
            let mut attrs = Attrs::new(child)?;
            let field = attrs.need("attributeName")?;
            ensure!(
                matches!(field, "start" | "end"),
                "unsupported text animator property {field:?}"
            );
            put(&mut obj, field, scalar_track(child, attrs, Number)?)
        })?;
    }
    Ok(obj.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_entities_and_plain_or_styled_copy_survive_reading() -> Result<()> {
        let doc = decode(
            br##"<sequence background="#102030">
            <text><copy> Try <!--comment-->&amp;  go&#10;</copy></text>
            <text><copy>Try <span id="version">6.2</span> <span> now</span>!</copy></text>
        </sequence>"##,
        )?;
        assert_eq!(doc["background"]["color"], json!([16, 32, 48]));
        assert_eq!(doc["graphics"][0]["content"]["text"], " Try &  go\n");
        assert_eq!(
            doc["graphics"][1]["content"]["text"],
            json!([
                {"text":"Try "},{"id":"version","text":"6.2"},{"text":" "},{"text":" now"},{"text":"!"}
            ])
        );
        Ok(())
    }

    #[test]
    fn geometry_channels_and_placement_motion_have_independent_owners() -> Result<()> {
        let doc = decode(br#"<sequence>
          <rect height="24"><animate attributeName="corner-radius">
            <key frame="0" value="12"/><key frame="6" value="12 12 12 0"/></animate>
            <animate attributeName="width"><key frame="0" value="48"/></animate>
            <animate attributeName="translate-x"><key frame="3" value="-2" ease="cubic(0 0.2 0.8 1)"/></animate>
          </rect></sequence>"#)?;
        let layer = &doc["graphics"][0];
        assert_eq!(
            layer["content"]["corner_radius"][1]["value"],
            json!([12, 12, 12, 0])
        );
        assert_eq!(layer["content"]["width"][0]["frame"], 0);
        assert_eq!(
            layer["channels"]["x"][0],
            json!({"frame":3,"value":-2,"ease":{"cubic":[0,0.2,0.8,1]}})
        );
        Ok(())
    }

    #[test]
    fn stroke_trim_accepts_static_and_keyed_endpoints_independently() -> Result<()> {
        let doc = decode(
            br##"<sequence>
          <path d="M0 0H40" stroke="#ffee00" stroke-width="4" stroke-start="0.2">
            <animate attributeName="stroke-end"><key frame="2" value="0" ease="hold"/>
            <key frame="4" value="1"/></animate>
          </path></sequence>"##,
        )?;
        let content = &doc["graphics"][0]["content"];
        assert_eq!(content["stroke_start"], 0.2);
        assert_eq!(
            content["stroke_end"],
            json!([
                {"frame":2,"value":0,"ease":"hold"},{"frame":4,"value":1}
            ])
        );
        assert_eq!(content["stroke"]["width"], 4);
        Ok(())
    }

    #[test]
    fn spatial_attributes_and_curves_belong_to_their_own_layer() -> Result<()> {
        let doc = decode(
            br#"<sequence><transform-group position="5 -3" pivot="4.25 6.75" scale-x="2">
              <animate attributeName="rotation"><key frame="0" value="-12.5"/><key frame="11" value="720"/></animate>
              <animate attributeName="scale-y"><key frame="0" value="0.5"/></animate>
              <rect width="32" height="16" rotation="90.42" scale="0.84"/>
            </transform-group></sequence>"#,
        )?;
        let group = &doc["graphics"][0];
        assert_eq!(group["transform"], json!({"pivot":[4.25,6.75],"scale_x":2}));
        assert_eq!(group["position"], json!([5, -3]));
        assert_eq!(group["channels"]["rotation"][1]["value"], 720);
        assert_eq!(group["channels"]["scale_y"][0]["value"], 0.5);
        let child = &group["content"]["children"][0];
        assert_eq!(
            child["transform"],
            json!({"rotation_degrees":90.42,"scale":0.84})
        );
        assert_eq!(child["content"]["width"], 32);
        Ok(())
    }

    #[test]
    fn duplicate_unknown_and_invalid_tracks_identify_the_xml_element() {
        for (body, reason) in [
            (
                r#"<backdrop source="lens.wgsl" sample-radius="16" sigma="2"/>"#,
                "backdrop requires exactly one of sample-radius or sigma",
            ),
            (
                r#"<backdrop source="lens.wgsl"/>"#,
                "backdrop requires exactly one of sample-radius or sigma",
            ),
            (
                r#"<backdrop source="lens.wgsl" sample-radius="8"><param name="x" value="2"><key frame="0" value="4"/></param></backdrop>"#,
                "choose a parameter value or keyframes",
            ),
            (
                r#"<backdrop source="lens.wgsl" sigma="8"><param name="x" value="2"/></backdrop>"#,
                "parameters require a 2D backdrop",
            ),
            (
                r#"<backdrop source="lens.wgsl" sample-radius="8"><param name="x"/></backdrop>"#,
                "track requires at least one key",
            ),
            (
                r#"<footage><history frames="3" source="a.wgsl"/><history frames="4" source="b.wgsl"/></footage>"#,
                "multiple definitions",
            ),
            (
                r#"<footage><history source="a.wgsl"/></footage>"#,
                "missing attribute frames",
            ),
            (
                r#"<rect><filter source="ink.wgsl"><matte name="shape" object="title"/></filter></rect>"#,
                "object mattes require a filter on a scene3d",
            ),
            (
                r#"<scene3d><filter source="ink.wgsl"><matte name="shape"/></filter></scene3d>"#,
                "missing attribute object",
            ),
            (
                r#"<scene3d><filter source="ink.wgsl"><matte name="shape" object="title" mode="blend"/></filter></scene3d>"#,
                "unknown attribute mode",
            ),
            (
                r#"<rect><rgb-grade gain="1 1"/></rect>"#,
                "expected three RGB coefficients",
            ),
            (
                r#"<rect><rgb-grade offset="0 0 0"><animate attributeName="offset"><key frame="0" value="1 1 1"/></animate></rgb-grade></rect>"#,
                "multiple definitions",
            ),
            (
                r#"<rect><rgb-grade><animate attributeName="gain"><key frame="0" value="1 NaN 1"/></animate></rgb-grade></rect>"#,
                "number must be finite",
            ),
            (
                r#"<rect><rgb-grade><animate attributeName="sigma"><key frame="0" value="1"/></animate></rgb-grade></rect>"#,
                "unknown animation property",
            ),
            (
                r#"<footage><source-blur sigma="20"><animate attributeName="sigma"><key frame="0" value="16"/></animate></source-blur></footage>"#,
                "multiple definitions",
            ),
            (
                r#"<footage><source-blur sigma="2"><focus-band center-y="3" half-width="1" feather="8"/><focus-band center-y="2" half-width="3" feather="4"/></source-blur></footage>"#,
                "multiple definitions",
            ),
            (
                r#"<footage><source-blur sigma="20"><animate attributeName="center-y"><key frame="0" value="5"/></animate></source-blur></footage>"#,
                "unknown animation property",
            ),
            (
                r#"<rect rotation="0"><animate attributeName="rotation"><key frame="0" value="40"/></animate></rect>"#,
                "multiple definitions",
            ),
            (
                r#"<rect scale-x="1"><motion><key frame="0" scale="1"/></motion></rect>"#,
                "multiple definitions",
            ),
            (
                r##"<path stroke="#ffffff" stroke-width="2" stroke-start="0"><animate attributeName="stroke-start"><key frame="0" value="1"/></animate></path>"##,
                "multiple definitions",
            ),
            (r#"<path stroke-end="1.01"/>"#, "between 0 and 1"),
            (
                r##"<path stroke="#ffffff" stroke-width="2"><animate attributeName="stroke-end"><key frame="0" value="-0.1"/></animate></path>"##,
                "between 0 and 1",
            ),
            (
                r#"<path d="M0 0H5" stroke-end="0.5"/>"#,
                "requires a stroke",
            ),
            (r#"<rect stroke-end="1"/>"#, "unknown attribute stroke-end"),
            (
                r#"<rect><animate attributeName="stroke-start"><key frame="0" value="0"/></animate></rect>"#,
                "unknown animation property",
            ),
            (
                r##"<image source="plate.png"><mask><path d="M0 0H9" stroke="#ffffff" stroke-width="2" stroke-end="0.5"/></mask></image>"##,
                "cannot trim a stroke",
            ),
            (
                r##"<rect stroke="#ffffff" stroke-width="5"><animate attributeName="stroke-width"><key frame="0" value="9"/></animate></rect>"##,
                "multiple definitions",
            ),
            (
                r#"<path d="M0 0 L1 1"><animate attributeName="stroke-width"><key frame="0" value="3"/></animate></path>"#,
                "requires a stroke",
            ),
            (
                r#"<rect width="12"><animate attributeName="width"><key frame="0" value="2"/></animate></rect>"#,
                "multiple definitions",
            ),
            (
                r#"<rect x="5" surprise="1"/>"#,
                "unknown attribute surprise",
            ),
            (
                r#"<rect><animate attributeName="widht"/></rect>"#,
                "unknown animation property",
            ),
            (r#"<rect corner-radius="1 2"/>"#, "requires one or four"),
            (r#"<rect corner-radius="-1"/>"#, "must be nonnegative"),
            (
                r#"<rect><animate attributeName="width"><key frame="2" value="1"/><key frame="2" value="2"/></animate></rect>"#,
                "strictly increase",
            ),
            (
                r#"<text><copy><span>oops<br/></span></copy></text>"#,
                "literal text",
            ),
            (
                r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
                "unexpected XML namespace",
            ),
        ] {
            let xml = format!("<sequence>\n{body}\n</sequence>");
            let error = format!("{:#}", decode(xml.as_bytes()).unwrap_err());
            assert!(
                error.contains("line 2,") && error.contains(reason),
                "{error}"
            );
        }
    }
}
