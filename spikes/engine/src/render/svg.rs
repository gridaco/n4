//! Explicit SVG artwork subset used in the creator studies.
use super::path::{StrokeCap, StrokeJoin, parse_geometry, rasterize, set_stroke};
use crate::{MAX_SURFACE_DIMENSION, graphics::Raster};
use anyhow::{Context, Result, bail, ensure};
use roxmltree::Node;
use skia_safe::{Canvas, Color, Paint, Shader, TileMode};
use std::{collections::HashMap, fs, path::Path};

fn color(value: &str) -> Result<Color> {
    let raw = value
        .strip_prefix('#')
        .context("SVG color must use #RRGGBB")?;
    ensure!(raw.len() == 6, "SVG color must use #RRGGBB");
    let rgb = u32::from_str_radix(raw, 16).context("invalid SVG color")?;
    Ok(Color::from_rgb(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    ))
}

// The viewport determines the whole raster, even if paint covers only part of it.
fn with_viewport<T>(
    source: &Path,
    read: impl FnOnce(Node<'_, '_>, [u32; 2]) -> Result<T>,
) -> Result<T> {
    let svg =
        fs::read_to_string(source).with_context(|| format!("read SVG {}", source.display()))?;
    let doc = roxmltree::Document::parse(&svg).context("parse SVG XML")?;
    let root = doc.root_element();
    ensure!(root.tag_name().name() == "svg", "expected an SVG document");
    ensure!(
        root.attributes()
            .all(|a| matches!(a.name(), "width" | "height")),
        "unsupported SVG root attribute"
    );
    let dimension = |key| -> Result<u32> {
        let n = root
            .attribute(key)
            .context("SVG needs explicit pixel dimensions")?
            .parse()?;
        ensure!(
            (1..=MAX_SURFACE_DIMENSION).contains(&n),
            "invalid SVG dimension"
        );
        Ok(n)
    };
    read(root, [dimension("width")?, dimension("height")?])
}

pub(super) fn raster_size(source: &Path) -> Result<[u32; 2]> {
    with_viewport(source, |_, size| Ok(size))
}

// The original SVG stays external. Reject unsupported presentation explicitly.
pub(crate) fn paint_svg(source: &Path) -> Result<Raster> {
    with_viewport(source, |root, size| {
        let gradients = gradients(root)?;
        rasterize(size, [0, 0], |canvas| {
            draw_children(canvas, root, &gradients)
        })
    })
}

fn attributes(node: Node<'_, '_>, allowed: &[&str]) -> Result<()> {
    for a in node.attributes() {
        ensure!(
            a.namespace().is_none() && allowed.contains(&a.name()),
            "unsupported SVG {} attribute {}",
            node.tag_name().name(),
            a.name()
        );
    }
    Ok(())
}

fn unit(value: &str) -> Result<f32> {
    let n: f32 = if let Some(pct) = value.strip_suffix('%') {
        pct.parse::<f32>()? / 100.
    } else {
        value.parse()?
    };
    ensure!((0. ..=1.).contains(&n), "expected an SVG fraction in [0,1]");
    Ok(n)
}

fn gradients(root: Node<'_, '_>) -> Result<HashMap<String, Shader>> {
    let mut map = HashMap::new();
    for defs in root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "defs")
    {
        attributes(defs, &[])?;
        for node in defs.children().filter(|n| n.is_element()) {
            ensure!(
                node.tag_name().name() == "linearGradient",
                "unsupported SVG definition {}",
                node.tag_name().name()
            );
            attributes(node, &["id", "gradientUnits", "x1", "y1", "x2", "y2"])?;
            ensure!(
                node.attribute("gradientUnits") == Some("userSpaceOnUse"),
                "gradient requires explicit userSpaceOnUse coordinates"
            );
            let point = |key| -> Result<f32> {
                let n: f32 = node
                    .attribute(key)
                    .context("gradient needs x1, y1, x2, y2")?
                    .parse()?;
                ensure!(n.is_finite(), "gradient coordinate must be finite");
                Ok(n)
            };
            let points = ((point("x1")?, point("y1")?), (point("x2")?, point("y2")?));
            ensure!(points.0 != points.1, "degenerate gradient direction");
            let mut colors = Vec::new();
            let mut offsets = Vec::new();
            for stop in node.children().filter(|n| n.is_element()) {
                ensure!(
                    stop.tag_name().name() == "stop" && !stop.children().any(|n| n.is_element()),
                    "expected SVG gradient stop"
                );
                attributes(stop, &["offset", "stop-color", "stop-opacity"])?;
                let t = unit(
                    stop.attribute("offset")
                        .context("gradient needs explicit stop offsets")?,
                )?;
                ensure!(
                    offsets.last().is_none_or(|prev| *prev <= t),
                    "expected ordered gradient stops"
                );
                offsets.push(t);
                let c = color(stop.attribute("stop-color").unwrap_or("#000000"))?;
                colors.push(c.with_a(
                    (unit(stop.attribute("stop-opacity").unwrap_or("1"))? * 255.).round() as u8,
                ));
            }
            ensure!(colors.len() >= 2, "gradient needs at least two stops");
            let shader = Shader::linear_gradient(
                points,
                colors.as_slice(),
                offsets.as_slice(),
                TileMode::Clamp,
                None,
                None,
            )
            .context("create linear gradient")?;
            let id = node.attribute("id").context("gradient needs an id")?;
            ensure!(
                !id.is_empty() && map.insert(id.to_owned(), shader).is_none(),
                "invalid or duplicate gradient id"
            );
        }
    }
    Ok(map)
}

fn pigment(brush: &mut Paint, value: &str, gradients: &HashMap<String, Shader>) -> Result<()> {
    if let Some(id) = value
        .strip_prefix("url(#")
        .and_then(|v| v.strip_suffix(')'))
    {
        brush.set_color(Color::WHITE);
        brush.set_shader(
            gradients
                .get(id)
                .with_context(|| format!("unknown SVG gradient {id}"))?
                .clone(),
        );
    } else {
        brush.set_shader(None);
        brush.set_color(color(value)?);
    }
    Ok(())
}

fn draw_children(
    canvas: &Canvas,
    parent: Node<'_, '_>,
    gradients: &HashMap<String, Shader>,
) -> Result<()> {
    for node in parent.children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "defs" if parent.tag_name().name() == "svg" => {}
            "path" => draw_path(canvas, node, gradients)?,
            "g" => {
                attributes(node, &["id", "opacity"])?;
                let opacity = unit(node.attribute("opacity").unwrap_or("1"))?;
                if opacity < 1. {
                    canvas.save_layer_alpha_f(None, opacity);
                    let painted = draw_children(canvas, node, gradients);
                    canvas.restore();
                    painted?;
                } else {
                    draw_children(canvas, node, gradients)?;
                }
            }
            other => bail!("unsupported SVG element {other}"),
        }
    }
    Ok(())
}

fn draw_path(
    canvas: &Canvas,
    node: Node<'_, '_>,
    gradients: &HashMap<String, Shader>,
) -> Result<()> {
    ensure!(
        !node.children().any(|n| n.is_element()),
        "unsupported child of SVG path"
    );
    attributes(
        node,
        &[
            "id",
            "d",
            "fill",
            "stroke",
            "stroke-width",
            "stroke-linecap",
            "stroke-linejoin",
        ],
    )?;
    let path = parse_geometry(node.attribute("d").context("SVG path needs d")?)?;
    let mut brush = Paint::default();
    brush.set_anti_alias(true);
    let fill = node.attribute("fill").unwrap_or("#000000");
    if fill != "none" {
        pigment(&mut brush, fill, gradients)?;
        canvas.draw_path(&path, &brush);
    }
    if let Some(stroke) = node.attribute("stroke").filter(|s| *s != "none") {
        let sw: f32 = node.attribute("stroke-width").unwrap_or("1").parse()?;
        ensure!(sw.is_finite() && sw >= 0., "invalid stroke-width");
        if sw == 0. {
            return Ok(());
        }
        let cap = match node.attribute("stroke-linecap").unwrap_or("butt") {
            "butt" => StrokeCap::Butt,
            "round" => StrokeCap::Round,
            "square" => StrokeCap::Square,
            other => bail!("unsupported stroke-linecap {other}"),
        };
        let join = match node.attribute("stroke-linejoin").unwrap_or("miter") {
            "miter" => StrokeJoin::Miter,
            "round" => StrokeJoin::Round,
            "bevel" => StrokeJoin::Bevel,
            other => bail!("unsupported stroke-linejoin {other}"),
        };
        pigment(&mut brush, stroke, gradients)?;
        set_stroke(&mut brush, sw, cap, join);
        canvas.draw_path(&path, &brush);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_stroke_and_group_opacity_survive_import() -> Result<()> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/stereo-field.svg");
        let raster = paint_svg(&src)?;
        let pixel = |x: usize, y: usize| &raster.rgba[(y * 680 + x) * 4..(y * 680 + x) * 4 + 4];
        let foreground = pixel(330, 100);
        // Both filled paths cover this point. Applying opacity to each shape
        // independently would give ~224 alpha, rather than one group at 65%.
        assert!((165..=166).contains(&foreground[3]));
        assert_ne!(pixel(20, 100), pixel(659, 100)); // Different points on one authored gradient.
        assert_eq!(pixel(20, 1), &[0; 4]);
        assert!(raster.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255)); // Opaque thin stroke.
        Ok(())
    }
}
