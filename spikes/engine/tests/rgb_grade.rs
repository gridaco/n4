//! Source-space grade, order and per-instance ownership on the real compositor.
use anyhow::Result;
use n4_engine::{Engine, Frame, Project, session::Session};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write_strip(path: &Path, pixels: &[[u8; 4]]) -> Result<()> {
    let mut encoder = png::Encoder::new(fs::File::create(path)?, pixels.len() as u32, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels.as_flattened())?;
    writer.finish()?;
    Ok(())
}

fn project(width: usize, layers: Value) -> Value {
    json!({"size":[width, 2],"fps":[30,1],"shots":[
        {"duration":4,"background":{"color":[40,80,120]},"graphics":layers}
    ]})
}

fn decode(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}

fn assert_pixel(frame: &Frame, x: usize, y: usize, want: [u8; 3]) {
    let i = 4 * (y * frame.size[0] as usize + x);
    let got = &frame.rgba[i..i + 4];
    assert!(
        got[..3].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2) && got[3] == 255,
        "at ({x},{y}) got {got:?}, expected {want:?}"
    );
}

#[test]
fn grade_matches_encoded_patches_preserves_alpha_and_belongs_to_one_placement() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let plate = dir.path().join("paper.png");
    let mut pixels: Vec<_> = (0..=255).map(|v| [v, v, v, 255]).collect();
    pixels.extend([[250, 40, 118, 255], [0, 128, 255, 128], [255, 0, 127, 0]]);
    write_strip(&plate, &pixels)?;
    let edit = |y, effects| {
        json!({"content":{"type":"image","source":"paper.png"},
                                   "position":[0,y],"effects":effects})
    };
    let gain = [1., 0.96, 0.84];
    let offset = [-39. / 255., -33. / 255., -21. / 255.];
    let grade = json!({"type":"rgb_grade","gain":gain,"offset":offset});
    let overlay =
        json!({"content":{"type":"rect","x":10,"width":8,"height":1,"fill":[255,220,0,255]}});
    let file = dir.path().join("edit.json");
    fs::write(
        &file,
        serde_json::to_vec(&project(
            pixels.len(),
            json!([edit(0, json!([grade])), edit(1, json!([])), overlay]),
        ))?,
    )?;
    let mut session = Session::open(&file)?;
    let frame = session.render(1)?;
    assert_eq!(frame.trace.graphics.source_effect_intermediate_passes, 0);
    assert_eq!(frame.trace.graphics.source_blur_scratch_bytes, 0);
    for (x, p) in pixels.iter().enumerate() {
        let alpha = f64::from(p[3]) / 255.;
        let rgb = |graded| {
            std::array::from_fn(|c| {
                let source = f64::from(p[c]) / 255.;
                let color = if graded {
                    (gain[c] * source + offset[c]).clamp(0., 1.)
                } else {
                    source
                };
                let dest = decode(f64::from([40, 80, 120][c]) / 255.);
                (255. * encode(decode(color) * alpha + dest * (1. - alpha))).round() as u8
            })
        };
        assert_pixel(
            &frame,
            x,
            0,
            if (10..18).contains(&x) {
                [255, 220, 0]
            } else {
                rgb(true)
            },
        );
        assert_pixel(&frame, x, 1, rgb(false));
    }
    session.set(
        session.revision(),
        "/shots/0/graphics/0/effects/0/offset/2",
        json!(0.4),
    )?;
    let warmed = session.render(1)?;
    assert_ne!(warmed.rgba, frame.rgba);
    let second_row = pixels.len() * 4;
    assert_eq!(warmed.rgba[second_row..], frame.rgba[second_row..]);
    assert_pixel(&warmed, 14, 0, [255, 220, 0]);
    session.undo(session.revision())?;
    assert_eq!(session.render(1)?.rgba, frame.rgba);
    write_strip(&plate, &vec![[255, 0, 0, 255]; pixels.len()])?;
    session.refresh_assets(session.revision())?;
    let replaced = session.render(1)?;
    assert_pixel(&replaced, 0, 0, [216, 0, 0]);
    assert_pixel(&replaced, 0, 1, [255, 0, 0]);
    Ok(())
}

#[test]
fn reversing_grade_and_gaussian_changes_the_edge_and_respects_each_clamp() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("edge.png");
    let pixels: Vec<_> = (0..17)
        .map(|x| if x < 8 { [0, 0, 0, 255] } else { [255; 4] })
        .collect();
    write_strip(&path, &pixels)?;
    let blur = json!({"type":"source_blur","sigma":1});
    let grade = json!({"type":"rgb_grade","gain":[2,2,2]});
    let down = json!({"type":"rgb_grade","gain":[0.5,0.5,0.5]});
    let draw = |effects: Value| -> Result<Frame> {
        let layers = json!([{"content":{"type":"image","source":path},"effects":effects}]);
        Engine::new(serde_json::from_value::<Project>(project(17, layers))?)?.render(0)
    };
    let before = draw(json!([grade, blur]))?;
    let after = draw(json!([blur, grade]))?;
    let both = draw(json!([grade, blur, grade, down]))?;
    assert_eq!(before.trace.graphics.source_effect_intermediate_passes, 1);
    assert_eq!(before.trace.graphics.source_blur_scratch_bytes, 17 * 24);
    assert_eq!(after.trace.graphics.source_effect_intermediate_passes, 0);
    assert_eq!(after.trace.graphics.source_blur_scratch_bytes, 17 * 16);
    let weight = |i: i32| (-0.5 * f64::from(i * i)).exp();
    let sum: f64 = (-3..=3).map(weight).sum();
    for x in 0..17 {
        let bright: f64 = (-3..=3).filter(|i| x + i >= 8).map(weight).sum();
        let mixed = encode(bright / sum);
        let boosted = (2. * mixed).clamp(0., 1.);
        for (frame, expected) in [(&before, mixed), (&after, boosted), (&both, boosted * 0.5)] {
            assert_pixel(frame, x as usize, 0, [(255. * expected).round() as u8; 3]);
        }
    }
    Ok(())
}

#[test]
fn keyed_rgb_uses_the_placements_local_clock_after_nonsequential_seeks() -> Result<()> {
    let grade = json!({"type":"rgb_grade","gain":[
        {"frame":0,"value":[1,0,0]}, {"frame":2,"value":[0,0,1]}
    ]});
    // The downstream blur uses the same scratch texture across visits. A cache
    // keyed only on that handle would return a previous grade after seeking.
    let layer = json!({"span":[1,4],"content":{"type":"rect","width":12,"height":12,
                      "x":-4,"y":-5,"fill":[255,255,255,255]},
                      "effects":[grade,{"type":"source_blur","sigma":1}]});
    let mut edit = Engine::new(serde_json::from_value(project(4, json!([layer])))?)?;
    for (f, rgb) in [
        (3, [0, 0, 255]),
        (0, [40, 80, 120]),
        (2, [128, 0, 128]),
        (1, [255, 0, 0]),
        (3, [0, 0, 255]),
    ] {
        assert_pixel(&edit.render(f)?, 2, 1, rgb);
    }
    Ok(())
}

#[test]
fn a_moving_source_is_regraded_on_seek_and_the_identity_matches_the_decode() -> Result<()> {
    let movie = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    let open = |effects: Value| -> Result<Engine> {
        let p = json!({"size":[641,360],"fps":[30,1],"shots":[{
            "duration":3,"source":movie,"source_in":12,
            "graphics":[{"content":{"type":"footage"},"effects":effects}]
        }]});
        Engine::new(serde_json::from_value(p)?)
    };
    let mut raw = open(json!([]))?;
    let mut neutral = open(json!([{"type":"rgb_grade"}]))?;
    let mut graded = open(json!([{"type":"rgb_grade","gain":[0.5,1,1]}]))?;
    for t in [1, 0, 2, 1] {
        let original = raw.render(t)?;
        assert_eq!(neutral.render(t)?.rgba, original.rgba);
        let corrected = graded.render(t)?;
        for (x, y) in [(16, 344), (27, 344), (134, 112), (307, 236)] {
            let i = 4 * (y * 641 + x);
            let p = &original.rgba[i..i + 4];
            assert_pixel(&corrected, x, y, [p[0].div_ceil(2), p[1], p[2]]);
        }
    }
    Ok(())
}

#[test]
fn grade_on_an_isolated_group_receives_the_finished_children_and_preserves_empty_pixels()
-> Result<()> {
    let layers = json!([{"effects":[{"type":"rgb_grade","gain":[2,2,2],"offset":[0,0,0.1]}],
    "content":{"type":"group","children":[
        {"content":{"type":"rect","width":4,"height":2,"fill":[0,0,0,255]}},
        {"content":{"type":"rect","x":2,"width":2,"height":2,"fill":[255,255,255,128]}}
    ]}}]);
    let mut engine = Engine::new(serde_json::from_value(project(6, layers))?)?;
    let frame = engine.render(0)?;
    assert_pixel(&frame, 0, 0, [0, 0, 26]);
    assert_pixel(&frame, 3, 0, [255, 255, 255]);
    assert_pixel(&frame, 5, 0, [40, 80, 120]);
    Ok(())
}

#[test]
fn admission_budgets_the_intermediate_required_by_grade_before_blur() -> Result<()> {
    let grade = json!({"type":"rgb_grade","offset":[0.1,0,0]});
    let blur = json!({"type":"source_blur","sigma":1});
    let make = |effects| -> Result<Project> {
        Ok(serde_json::from_value(project(
            4,
            json!([{
                "content":{"type":"rect","width":3600,"height":3600},"effects":effects
            }]),
        ))?)
    };
    let mut engine = Engine::new(serde_json::from_value(project(4, json!([])))?)?;
    engine.replace_project(make(json!([blur, grade]))?)?;
    let error = engine
        .replace_project(make(json!([grade, blur]))?)
        .unwrap_err();
    assert!(format!("{error:#}").contains("256 MiB"), "{error:#}");
    Ok(())
}
