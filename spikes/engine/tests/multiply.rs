use anyhow::Result;
use n4_engine::{Engine, Frame, Project};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn pixel(frame: &Frame, x: usize, y: usize, expected: [u8; 3]) {
    let start = 4 * (y * frame.size[0] as usize + x);
    let rgba = &frame.rgba[start..start + 4];
    for (actual, expected) in rgba[..3].iter().zip(expected) {
        assert!(
            actual.abs_diff(expected) <= 2,
            "at ({x},{y}): {rgba:?}, expected channel {expected}"
        );
    }
    assert_eq!(rgba[3], 255);
}

fn engine(graphics: Value) -> Result<Engine> {
    Engine::new(serde_json::from_value(json!({
        "size":[32,16], "fps":[30,1],
        "shots":[{"duration":2,"background":{"color":[0,255,0]},"graphics":graphics}]
    }))?)
}

#[test]
fn multiply_uses_the_current_group_destination_including_its_alpha() -> Result<()> {
    let red = json!({"content":{"type":"path","d":"M0 0H8V8H0Z","fill":[255,0,0,255]},
                     "channels":{"opacity":[{"frame":0,"value":0.5}]}});
    let blue = json!({"content":{"type":"path","d":"M0 0H16V8H0Z","fill":[0,0,255,255]},
        "channels":{"opacity":[{"frame":0,"value":0.5}]},"blend":"multiply",
        "mask":{"position":[4,0],"content":{"type":"path","d":"M0.5 0H12V8H0.5Z"}}});
    let group = |kind| json!({"position":[3,2],"content":{"type":kind,"children":[red, blue]}});
    let isolated = engine(json!([group("group")]))?.render(0)?;
    let spatial = engine(json!([group("transform_group")]))?.render(0)?;
    // With two half-transparent primaries, source-over Multiply gives a
    // quarter of each non-overlapping color and 3/4 total alpha inside the
    // isolated group. Green behind the group supplies the remaining quarter.
    // 1/4 and 1/2 in linear light encode to 137 and 188 in sRGB.
    pixel(&isolated, 9, 5, [137, 137, 137]);
    pixel(&spatial, 9, 5, [137, 137, 0]);
    // Half mask coverage times half layer opacity gives 1/4 source alpha.
    pixel(&isolated, 7, 5, [165, 165, 99]);
    pixel(&spatial, 7, 5, [165, 165, 0]);
    // Where the destination is transparent, the blue source is preserved.
    pixel(&isolated, 14, 5, [0, 188, 188]);
    pixel(&spatial, 14, 5, [0, 188, 0]);
    for frame in [&isolated, &spatial] {
        pixel(frame, 4, 5, [188, 188, 0]); // Outside the blue layer's mask.
        pixel(frame, 20, 5, [0, 255, 0]);
    }
    Ok(())
}

#[test]
fn masked_filtered_multiply_group_casts_its_own_shadow() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let shader = directory.path().join("yellow.wgsl");
    fs::write(
        &shader,
        "fn effect(p:vec2<i32>, c:vec4<f32>, a:f32) -> vec4<f32> { return vec4(c.a,c.a,0.,c.a); }",
    )?;
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/gaussian-blur.wgsl");
    let layer = json!({
        "position":[4,2],"blend":"multiply",
        "mask":{"content":{"type":"path","d":"M2 0H8V5H2Z"}},
        "filter":{"shader":shader,"amount":100},
        "underlay":{"shader":kernel,"sigma_px":0,"offset":[8,0],"rgba":[255,0,0,255]},
        "content":{"type":"group","children":[
            {"content":{"type":"path","d":"M0 0H8V5H0Z","fill":[0,0,255,255]}}
        ]}
    });
    let mut body = json!({"size":[32,16],"fps":[30,1],"shots":[{
        "duration":1,"background":{"color":[208,128,101]},"graphics":[layer]
    }]});
    let mut edit = Engine::new(serde_json::from_value(body.clone())?)?;
    let frame = edit.render(0)?;
    pixel(&frame, 8, 4, [208, 128, 0]); // The yellow point filter ran before Multiply.
    pixel(&frame, 15, 4, [255, 0, 0]); // The offset samples the group's covered alpha.
    pixel(&frame, 13, 4, [208, 128, 101]); // No shadow from the gap in the mask.
    pixel(&frame, 5, 4, [208, 128, 101]); // Unpainted source edge.

    // With an offset of zero, the mark also sees its own already-painted
    // underlay. A destination snapshot taken before the shadow misses it.
    body["shots"][0]["graphics"][0]["underlay"]["offset"] = json!([0, 0]);
    let frame = Engine::new(serde_json::from_value(body)?)?.render(0)?;
    pixel(&frame, 8, 4, [255, 0, 0]);
    Ok(())
}

#[test]
fn two_marks_follow_moving_footage_and_an_intervening_layer_after_seeking() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    fs::copy(source, root.path().join("picture.mkv"))?;
    fs::create_dir(root.path().join("sequences"))?;
    let master = root.path().join("project.proj");
    fs::write(
        &master,
        "project ink fps 30/1 size 641x360\n[0,3) V1 shot\n",
    )?;
    fs::write(
        root.path().join("sequences/shot.xml"),
        r##"
        <sequence source="picture.mkv" source-in="12">
          <rect x="10" y="338" width="30" height="13" fill="#ff8000" blend="multiply" />
          <rect x="26" y="338" width="13" height="13" fill="#8040c0" />
          <rect x="10" y="338" width="30" height="13" fill="#80ffff" blend="multiply" />
          <rect x="10" y="338" width="2" height="13" fill="#ffffff" />
        </sequence>"##,
    )?;
    let mut edit = Engine::new(Project::load(&master)?)?;
    // The first cell of the movie's barcode switches between black and white
    // each frame. Both marks must read the picture at the requested time.
    let white_cell = edit.render(1)?;
    pixel(&white_cell, 16, 344, [128, 128, 0]);
    let black_cell = edit.render(0)?;
    pixel(&black_cell, 16, 344, [0, 0, 0]);
    for frame in [&black_cell, &white_cell] {
        // Decode sRGB(128) to linear 0.21586. Its square encodes to 61.
        pixel(frame, 32, 344, [61, 64, 192]);
        pixel(frame, 11, 344, [255, 255, 255]);
        let stats = &frame.trace.graphics;
        // Two 30x13 marks, allowing up to two pixels of antialias padding on each edge.
        assert!((2 * 30 * 13..=2 * 34 * 17).contains(&stats.multiply_pixels));
        assert_eq!(stats.multiply_copy_bytes, 4 * stats.multiply_pixels);
    }
    let revisit = edit.render(1)?;
    assert_eq!(revisit.rgba, white_cell.rgba);
    assert_eq!(revisit.trace.graphics.graphic_uploads, 0);
    assert!(edit.render(1)?.trace.reused_decode);
    Ok(())
}

#[test]
fn explicit_footage_with_multiply_is_composited_at_its_paint_position() -> Result<()> {
    let media = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    let make = |graphics| -> Result<Engine> {
        Engine::new(serde_json::from_value(
            json!({"size":[641,360],"fps":[30,1],
            "shots":[{"source":media,"source_in":13,"duration":1,"graphics":graphics}]}),
        )?)
    };
    let footage = json!({"blend":"multiply","content":{"type":"footage"}});
    let black_stage = make(json!([footage]))?.render(0)?;
    pixel(&black_stage, 16, 344, [0, 0, 0]); // Normal backmost footage can skip its draw; Multiply cannot.
    let red = json!({"content":{"type":"path","d":"M10 338H38V351H10Z","fill":[255,0,0,255]}});
    let over_red = make(json!([red, footage]))?.render(0)?;
    pixel(&over_red, 16, 344, [255, 0, 0]); // Source frame 13 has a white low bit.
    pixel(&over_red, 32, 344, [0, 0, 0]); // Its second bit is black.
    Ok(())
}

#[test]
fn invisible_marks_leave_the_picture_and_copy_counters_untouched() -> Result<()> {
    let path = json!({"type":"path","d":"M0 0H8V8H0Z","fill":[0,0,0,255]});
    let mut edit = engine(json!([
        {"content":path,"blend":"multiply","position":[40,0]},
        {"content":path,"blend":"multiply","channels":{"opacity":[{"frame":0,"value":0}]}},
        {"content":path,"blend":"multiply","span":[1,2]}
    ]))?;
    let invisible = edit.render(0)?;
    let baseline = engine(json!([]))?.render(0)?;
    assert_eq!(invisible.rgba, baseline.rgba);
    assert_eq!(invisible.trace.graphics.multiply_pixels, 0);
    assert_eq!(invisible.trace.graphics.multiply_copy_bytes, 0);
    let visible = edit.render(1)?;
    pixel(&visible, 4, 4, [0, 0, 0]);
    assert!(visible.trace.graphics.multiply_copy_bytes > 0);
    Ok(())
}

#[test]
fn multiply_requires_a_drawable_source() {
    let adjustment = json!({"type":"backdrop","filter":{"shader":"filter.wgsl","sigma_px":1}});
    let mask = json!({"content":{"type":"path","d":"M0 0H4V4H0Z"}});
    let spatial = json!({"type":"transform_group","children":[]});
    for (layer, expected) in [
        (
            json!({"content":adjustment,"mask":mask,"blend":"multiply"}),
            "backdrop adjustment",
        ),
        (
            json!({"content":spatial,"blend":"multiply"}),
            "spatial motion only",
        ),
    ] {
        let error = engine(json!([layer]))
            .err()
            .expect("invalid blend accepted");
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
}
