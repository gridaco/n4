use super::*;

pub(super) fn light(node: Element) -> Result<Value> {
    let mut light = simple(
        node,
        &[
            ("direction", "direction", Triple),
            ("intensity", "intensity", Number),
            ("color", "color", Rgb),
        ],
    )?;
    let obj = light.as_object_mut().unwrap();
    obj.entry("intensity").or_insert(json!(1));
    obj.entry("color").or_insert(json!([255, 255, 255]));
    Ok(light)
}

pub(super) fn rect_light(node: Element) -> Result<Value> {
    let mut light = simple(
        node,
        &[
            ("center", "center", Triple),
            ("aim", "aim", Triple),
            ("size", "size", Pair),
            ("intensity", "intensity", Number),
            ("color", "color", Rgb),
        ],
    )?;
    let obj = light.as_object_mut().unwrap();
    ensure!(
        ["center", "aim", "size"]
            .iter()
            .all(|k| obj.contains_key(*k)),
        "rect-light requires center, aim and size"
    );
    obj.entry("intensity").or_insert(json!(1));
    obj.entry("color").or_insert(json!([255, 255, 255]));
    Ok(light)
}

pub(super) fn ambient(node: Element) -> Result<Value> {
    let mut light = simple(
        node,
        &[("intensity", "intensity", Number), ("color", "color", Rgb)],
    )?;
    let obj = light.as_object_mut().unwrap();
    obj.entry("intensity").or_insert(json!(1));
    obj.entry("color").or_insert(json!([255, 255, 255]));
    Ok(light)
}

pub(super) fn path_trace(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut trace = Object::new();
    attrs.read(
        &mut trace,
        &[
            ("samples", "samples", Number),
            ("bounces", "bounces", Number),
            ("clamp", "clamp", Number),
            ("seed", "seed", Number),
        ],
    )?;
    let denoise = match attrs.take("denoise") {
        None | Some("true") => true,
        Some("false") => false,
        Some(other) => bail!("path-trace denoise expects true or false, found {other:?}"),
    };
    attrs.finish()?;
    ensure!(children(node)?.is_empty(), "path-trace has no children");
    trace.insert("denoise".into(), denoise.into());
    for (key, value) in [
        ("samples", json!(64)),
        ("bounces", json!(8)),
        ("clamp", json!(10)),
        ("seed", json!(0)),
    ] {
        trace.entry(key).or_insert(value);
    }
    Ok(trace.into())
}

pub(super) fn instance(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut model = Object::new();
    attrs.read(
        &mut model,
        &[
            ("id", "id", Str),
            ("source", "source", Str),
            ("prim", "prim", Str),
            ("position", "position", Triple),
            ("rotation-x", "rotation_x", Number),
            ("rotation-y", "rotation_y", Number),
            ("rotation-z", "rotation_z", Number),
        ],
    )?;
    attrs.finish()?;
    for child in children(node)? {
        located(child, || match child.tag_name().name() {
            "animate" => curve(&mut model, child, &ROTATIONS),
            "part" => append(&mut model, "parts", part(child)?),
            "surface" => append(&mut model, "surfaces", surface(child)?),
            unknown => bail!("unknown USD child <{unknown}>"),
        })?;
    }
    Ok(model.into())
}

const ROTATIONS: [&str; 3] = ["rotation-x", "rotation-y", "rotation-z"];

fn append(obj: &mut Object, field: &str, value: Value) -> Result<()> {
    obj.entry(field)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .unwrap()
        .push(value);
    Ok(())
}

fn curve(obj: &mut Object, node: Element, allowed: &[&str]) -> Result<()> {
    tag(node, "animate")?;
    let mut attrs = Attrs::new(node)?;
    let name = attrs.need("attributeName")?;
    ensure!(allowed.contains(&name), "unknown spatial channel {name:?}");
    put(
        obj,
        &name.replace('-', "_"),
        scalar_track(node, attrs, Number)?,
    )
}

fn part(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut part = Object::new();
    part.insert("prim".into(), attrs.need("prim")?.into());
    attrs.read(
        &mut part,
        &[
            ("rotation-x", "rotation_x", Number),
            ("rotation-y", "rotation_y", Number),
            ("rotation-z", "rotation_z", Number),
        ],
    )?;
    attrs.finish()?;
    for child in children(node)? {
        located(child, || curve(&mut part, child, &ROTATIONS))?;
    }
    Ok(part.into())
}

fn surface(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut surface = Object::new();
    attrs.read(
        &mut surface,
        &[
            ("slot", "slot", Str),
            ("role", "role", Str),
            ("size", "size", Pair),
            ("emission", "emission", Number),
            ("glow", "glow", Number),
        ],
    )?;
    attrs.finish()?;
    let layers = children(node)?
        .into_iter()
        .map(graphic)
        .collect::<Result<Vec<_>>>()?;
    ensure!(!layers.is_empty(), "live surface requires artwork");
    surface.insert("layers".into(), layers.into());
    Ok(surface.into())
}

pub(super) fn studio(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut look = Object::new();
    attrs.read(
        &mut look,
        &[
            ("environment-rotation", "environment_rotation", Number),
            ("environment-intensity", "environment_intensity", Number),
            ("exposure", "exposure", Number),
            ("shadow-center", "shadow_center", Triple),
            ("shadow-extent", "shadow_extent", Number),
            ("shadow-bias", "shadow_bias", Number),
            ("shadow-softness", "shadow_softness", Number),
        ],
    )?;
    attrs.finish()?;
    for child in children(node)? {
        located(child, || {
            curve(
                &mut look,
                child,
                &["environment-rotation", "environment-intensity", "exposure"],
            )
        })?;
    }
    for (key, value) in [
        ("environment_rotation", json!(0)),
        ("environment_intensity", json!(1)),
        ("exposure", json!(0)),
        ("shadow_center", json!([0, 0, 0])),
        ("shadow_extent", json!(5)),
        ("shadow_bias", json!(0.003)),
        ("shadow_softness", json!(1)),
    ] {
        look.entry(key).or_insert(value);
    }
    Ok(look.into())
}

pub(super) fn plane(node: Element) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let mut plane = Object::new();
    attrs.read(
        &mut plane,
        &[("id", "id", Str), ("position", "position", Triple)],
    )?;
    attrs.finish()?;
    let kids = children(node)?;
    ensure!(
        kids.len() == 1 && kids[0].has_tag_name("text"),
        "plane requires one editable text element"
    );
    plane.insert("text".into(), graphic(kids[0])?);
    Ok(plane.into())
}

pub(super) fn filter(node: Element, is_scene: bool) -> Result<Value> {
    let mut attrs = Attrs::new(node)?;
    let shader = attrs.need("source")?;
    let amount = attrs
        .take("amount")
        .map(number)
        .transpose()?
        .unwrap_or(json!(100));
    attrs.finish()?;
    let mut filter = json!({"shader":shader,"amount":amount});
    let mut mattes = Vec::new();
    for child in children(node)? {
        mattes.push(located(child, || {
            tag(child, "matte")?;
            ensure!(is_scene, "object mattes require a filter on a scene3d");
            let mut attrs = Attrs::new(child)?;
            let mut matte = Object::new();
            matte.insert("name".into(), attrs.need("name")?.into());
            matte.insert("object".into(), attrs.need("object")?.into());
            attrs.read(
                &mut matte,
                &[
                    ("occluded-by", "occluded_by", Str),
                    ("coverage", "coverage", Token),
                ],
            )?;
            attrs.finish()?;
            ensure!(children(child)?.is_empty(), "matte has no children");
            Ok(Value::Object(matte))
        })?);
    }
    if !mattes.is_empty() {
        filter["mattes"] = mattes.into();
    }
    Ok(filter)
}
