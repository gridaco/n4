//! Source-local Gaussian behavior, measured against independent linear-light samples.
use anyhow::Result;
use n4_engine::{Engine, Frame, Project, session::Session};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn image(path: &Path, width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Result<()> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.extend(texel(x, y));
        }
    }
    let mut encoder = png::Encoder::new(fs::File::create(path)?, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgba)?;
    writer.finish()?;
    Ok(())
}

fn project(size: [u32; 2], graphics: Value, background: [u8; 3]) -> Value {
    json!({"size":size,"fps":[30,1],"shots":[{"duration":3,"background":{"color":background},"graphics":graphics}]})
}

fn weights(sigma: f64) -> Vec<(i32, f64)> {
    let radius = (3.0 * sigma).ceil() as i32;
    let mut weights: Vec<_> = (-radius..=radius)
        .map(|i| (i, (-0.5 * (f64::from(i) / sigma).powi(2)).exp()))
        .collect();
    let sum: f64 = weights.iter().map(|(_, w)| *w).sum();
    for (_, w) in &mut weights {
        *w /= sum;
    }
    weights
}

fn srgb(linear: f64) -> u8 {
    let x = linear.clamp(0., 1.);
    let encoded = if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1. / 2.4) - 0.055
    };
    (255. * encoded).round() as u8
}

fn assert_rgb(frame: &Frame, x: usize, y: usize, expected: [u8; 3]) {
    let i = (y * frame.size[0] as usize + x) * 4;
    let pixel = &frame.rgba[i..i + 4];
    for (&channel, want) in pixel.iter().zip(expected) {
        assert!(
            channel.abs_diff(want) <= 2,
            "at ({x},{y}) got {pixel:?}, expected {expected:?}"
        );
    }
    assert_eq!(pixel[3], 255);
}

#[test]
fn gaussian_spread_in_source_pixels_survives_camera_scale_at_production_radii() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let plate = dir.path().join("step.png");
    image(&plate, 129, 4, |x, _| {
        if x >= 64 { [255; 4] } else { [0, 0, 0, 255] }
    })?;
    for sigma in [20., 26.] {
        let layer = json!({"content":{"type":"image","source":plate},"effects":[{"type":"source_blur","sigma":sigma}]});
        let group = json!({"transform":{"scale_x":2,"scale_y":2},
            "content":{"type":"transform_group","children":[layer]}});
        let body = project([258, 8], json!([group]), [0; 3]);
        let frame = Engine::new(serde_json::from_value(body)?)?.render(0)?;
        let kernel = weights(sigma);
        let edge = |src: i32| {
            kernel
                .iter()
                .filter(|(i, _)| (src + i).clamp(0, 128) >= 64)
                .map(|(_, w)| w)
                .sum::<f64>()
        };
        // Source texel centers lie halfway between adjacent output centers at 2×.
        for x in (19..239).step_by(5) {
            let local = (x as f64 + 0.5) / 2. - 0.5;
            let (base, t) = (local.floor() as i32, local.fract());
            let value = srgb(edge(base) * (1. - t) + edge(base + 1) * t);
            assert_rgb(&frame, x, 4, [value; 3]);
        }
    }
    Ok(())
}

#[test]
fn blur_precedes_the_layer_mask_and_composites_premultiplied_alpha() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let plate = dir.path().join("impulse.png");
    // Hidden cyan must contribute no light. The red impulse just outside the final
    // mask still contributes to its neighbor, at a quarter of the source opacity.
    image(&plate, 17, 17, |x, y| {
        if x == 8 && y == 8 {
            [255, 0, 0, 128]
        } else {
            [0, 255, 255, 0]
        }
    })?;
    let layer = json!({"content":{"type":"image","source":plate},"effects":[{"type":"source_blur","sigma":1.5}],
        "channels":{"opacity":[{"frame":0,"value":0.5}]},
        "mask":{"content":{"type":"path","d":"M9 0H17V17H9Z"}}});
    let top = json!({"content":{"type":"path","d":"M12 0H13V17H12Z","fill":[0,255,0,255]}});
    let frame = Engine::new(serde_json::from_value(project(
        [17, 17],
        json!([layer, top]),
        [0, 0, 255],
    ))?)?
    .render(0)?;
    let kernel = weights(1.5);
    let at = |i| kernel.iter().find(|(delta, _)| *delta == i).unwrap().1;
    for (x, y) in [(9, 8), (10, 8), (9, 10)] {
        let a = at(x - 8) * at(y - 8) * (128. / 255.) * 0.5;
        assert_rgb(&frame, x as usize, y as usize, [srgb(a), 0, srgb(1. - a)]);
    }
    assert_rgb(&frame, 8, 8, [0, 0, 255]);
    assert_rgb(&frame, 12, 8, [0, 255, 0]);
    Ok(())
}

#[test]
fn center_edits_reuse_the_blur_and_asset_and_sigma_edits_refresh_it() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let plate = dir.path().join("lines.png");
    image(&plate, 32, 32, |_, y| {
        if y % 2 == 0 { [255; 4] } else { [0, 0, 0, 255] }
    })?;
    let layer = json!({"content":{"type":"image","source":"lines.png"},"effects":[{"type":"source_blur","sigma":2,
        "focus_band":{"center_y":8,"half_width":2,"feather":4}}]});
    let file = dir.path().join("edit.json");
    fs::write(
        &file,
        serde_json::to_vec(&project([32, 32], json!([layer]), [0; 3]))?,
    )?;
    let mut edit = Session::open(&file)?;
    let original = edit.render(1)?;
    assert_eq!(original.trace.graphics.source_blur_passes, 2);
    assert_rgb(&original, 16, 8, [255; 3]);
    let path = "/shots/0/graphics/0/effects/0";
    edit.set(
        edit.revision(),
        &format!("{path}/focus_band/center_y"),
        json!(21),
    )?;
    let moved = edit.render(1)?;
    assert_eq!(moved.trace.graphics.source_blur_passes, 0);
    assert_eq!(moved.trace.graphics.source_blur_cache_hits, 1);
    assert_rgb(&moved, 16, 8, [188; 3]);
    assert_rgb(&moved, 16, 21, [0; 3]);
    edit.undo(edit.revision())?;
    assert_eq!(edit.render(1)?.rgba, original.rgba);
    edit.set(edit.revision(), &format!("{path}/sigma"), json!(3))?;
    assert_eq!(edit.render(1)?.trace.graphics.source_blur_passes, 2);
    image(&plate, 32, 32, |_, _| [255, 0, 0, 255])?;
    edit.refresh_assets(edit.revision())?;
    let red = edit.render(1)?;
    assert_eq!(red.trace.graphics.source_blur_passes, 2);
    assert_rgb(&red, 16, 21, [255, 0, 0]);
    Ok(())
}

#[test]
fn decoded_frame_and_zero_strength_bypass_follow_random_seeks() -> Result<()> {
    let movie = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    let load = |graphic: Value| -> Result<Engine> {
        let p: Project = serde_json::from_value(json!({"size":[641,360],"fps":[30,1],"shots":[
            {"duration":3,"source":movie,"source_in":12,"graphics":[graphic]}]}))?;
        Engine::new(p)
    };
    let source = json!({"content":{"type":"footage"}});
    let mut raw = load(source.clone())?;
    let make = |mut blur: Value| {
        let mut layer = source.clone();
        blur["type"] = json!("source_blur");
        layer["effects"] = json!([blur]);
        layer
    };
    for blur in [json!({"sigma":0}), json!({"sigma":2,"amount":0})] {
        let mut off = load(make(blur))?;
        let a = off.render(1)?;
        assert_eq!(a.rgba, raw.render(1)?.rgba);
        assert_eq!(a.trace.graphics.source_blur_passes, 0);
    }
    let mut edit = load(make(json!({"sigma":1.5})))?;
    let first = edit.render(1)?;
    let prev = edit.render(0)?;
    assert!(
        first.trace.graphics.source_blur_passes > 0 && prev.trace.graphics.source_blur_passes > 0
    );
    assert_rgb(&first, 16, 344, [255; 3]);
    assert_rgb(&prev, 16, 344, [0; 3]);
    assert_ne!(first.rgba, raw.render(1)?.rgba);
    assert_eq!(edit.render(1)?.rgba, first.rgba);
    Ok(())
}

#[test]
fn a_blurred_video_inset_uses_its_own_trim_and_leaves_the_plate_visible() -> Result<()> {
    let movie = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    let inset = json!({"position":[2,2],"span":[1,3],"effects":[{"type":"source_blur","sigma":1.5}],
        "content":{"type":"video","source":movie,"start":1,"source_in":13,"duration":2,"size":[641,360]}});
    let p = serde_json::from_value(project([648, 364], json!([inset]), [255, 0, 255]))?;
    let mut edit = Engine::new(p)?;
    let before = edit.render(0)?;
    assert_rgb(&before, 18, 346, [255, 0, 255]);
    assert_eq!(before.trace.graphics.source_blur_passes, 0);
    let first = edit.render(1)?;
    let next = edit.render(2)?;
    assert_rgb(&first, 18, 346, [255; 3]);
    assert_rgb(&next, 18, 346, [0; 3]);
    assert_rgb(&next, 0, 0, [255, 0, 255]);
    assert_eq!(edit.render(1)?.rgba, first.rgba);
    Ok(())
}

#[test]
fn a_texel_inside_the_sharp_band_contributes_to_the_nearby_blurred_source() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let plate = dir.path().join("one-white-line.png");
    image(&plate, 16, 16, |_, y| {
        if y == 8 { [255; 4] } else { [0, 0, 0, 255] }
    })?;
    let band = json!({"center_y":8.5,"half_width":0.5,"feather":1});
    let blur = json!({"type":"source_blur","sigma":2,"amount":[{"frame":0,"value":0},{"frame":2,"value":1}],"focus_band":band});
    let layer = json!({"content":{"type":"image","source":plate},"effects":[blur]});
    let mut edit = Engine::new(serde_json::from_value(project(
        [16, 16],
        json!([layer]),
        [0; 3],
    ))?)?;
    let weight = |offset| {
        weights(2.)
            .into_iter()
            .find(|(i, _)| *i == offset)
            .unwrap()
            .1
    };
    let zero = edit.render(0)?;
    assert_eq!(zero.trace.graphics.source_blur_passes, 0);
    assert_rgb(&zero, 6, 9, [0; 3]);
    for (time, amount) in [(2, 1.), (1, 0.5)] {
        let output = edit.render(time)?;
        assert_rgb(&output, 6, 8, [255; 3]);
        // At y=9 the coverage smoothstep is halfway through its feather.
        assert_rgb(&output, 6, 9, [srgb(0.5 * amount * weight(1)); 3]);
        assert_rgb(&output, 6, 10, [srgb(amount * weight(2)); 3]);
    }
    Ok(())
}

#[test]
fn a_failed_frame_cannot_publish_an_unsubmitted_blur() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let insert = dir.path().join("insert.png");
    image(&insert, 2, 2, |_, _| [0, 255, 0, 255])?;
    let layers = json!([
        {"content":{"type":"rect","x":4,"y":4,"width":16,"height":16,"fill":[255,0,0,255]},
         "effects":[{"type":"source_blur","sigma":1.5}]},
        {"span":[0,1],"content":{"type":"image","source":insert}}
    ]);
    let p: Project = serde_json::from_value(project([24, 24], layers, [0; 3]))?;
    let mut reference = Engine::new(p.clone())?;
    let clean = reference.render(1)?;
    let with_insert = reference.render(0)?;
    assert_rgb(&clean, 12, 12, [255, 0, 0]);
    assert_rgb(&with_insert, 0, 0, [0, 255, 0]);

    let mut engine = Engine::new(p)?;
    // Simulate a partially replaced asset after opening but before its first draw.
    fs::write(&insert, b"truncated PNG")?;
    let error = engine
        .render(0)
        .err()
        .expect("expected the truncated image to fail");
    assert!(format!("{error:#}").contains("insert.png"), "{error:#}");
    let revisit = engine.render(1)?;
    assert_eq!(revisit.trace.graphics.source_blur_passes, 2);
    assert_eq!(revisit.rgba, clean.rgba);
    image(&insert, 2, 2, |_, _| [0, 255, 0, 255])?;
    assert_eq!(engine.render(0)?.rgba, with_insert.rgba);
    Ok(())
}

#[test]
fn scratch_admission_measures_each_shape_at_one_time() -> Result<()> {
    let position = json!([{"frame":0,"value":0},{"frame":2,"value":1_073_741_824}]);
    let moving_rect = json!({"type":"rect","x":position,"y":position,"width":16,"height":16,
                             "fill":[255,0,0,255]});
    let moving_curve = json!({"type":"path","d":{"keys":[
        {"frame":0,"points":[[0,8],[4,8],[12,8],[16,8]],"ease":"spline"},
        {"frame":2,"points":[[5000,5008],[5004,5008],[5012,5008],[5016,5008]]}
    ]},"fill":null,"stroke":{"width":8,"rgba":[255,0,0,255]}});
    let with_blur = |shape| -> Result<Project> {
        Ok(serde_json::from_value(project(
            [24, 24],
            json!([
                {"content":shape,"effects":[{"type":"source_blur","sigma":1.0}]}
            ]),
            [0; 3],
        ))?)
    };
    for shape in [moving_rect, moving_curve] {
        let mut engine = Engine::new(with_blur(shape)?)?;
        let before = engine.render(0)?;
        assert_rgb(&before, 8, 8, [255, 0, 0]);
        assert!(before.trace.graphics.source_blur_scratch_bytes < 16_384);

        // A genuinely oversized pose must still fail before installing this edit.
        let big = json!({"type":"rect","width":4300,"height":4300});
        let error = engine.replace_project(with_blur(big)?).unwrap_err();
        assert!(format!("{error:#}").contains("256 MiB"), "{error:#}");
        assert_eq!(engine.revision(), before.trace.revision);
        assert_eq!(engine.render(0)?.rgba, before.rgba);
    }
    Ok(())
}
