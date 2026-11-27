use anyhow::Result;
use n4_engine::{Engine, Project, graphics::Graphic, session::Session};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn edit(graphics: Value) -> Value {
    json!({
        "size":[40,32], "fps":[30,1],
        "shots":[{"duration":1,"background":{"color":[0,0,0]}},
                 {"duration":12,"background":{"color":[0,0,0]},"graphics":graphics}]
    })
}

fn engine(graphics: Value) -> Result<Engine> {
    Engine::new(serde_json::from_value::<Project>(edit(graphics))?)
}

#[test]
fn nested_local_motion_matches_the_composed_pose_without_resampling_each_parent() -> Result<()> {
    let artwork = json!({"type":"path","d":"M0 0H5V2H2V5H0Z","fill":[255,125,33,255]});
    let child = json!({
        "span":[1,6],"position":[2,1],"content":artwork,
        "motion":[{"frame":0,"opacity":0.25},{"frame":4,"x":2,"opacity":0.75}]
    });
    let inner = json!({
        "span":[1,8],"position":[4,1],
        "transform":{"pivot":[2,1],"rotation_degrees":-90,"scale":0.75},
        "motion":[{"frame":0},{"frame":6,"y":2}],
        "content":{"type":"transform_group","children":[child]}
    });
    let outer = json!({
        "span":[2,12],"position":[15,5],
        "transform":{"pivot":[1,2],"rotation_degrees":90,"scale":2},
        "motion":[{"frame":0},{"frame":8,"x":4.5,"y":2}],
        "content":{"type":"transform_group","children":[inner]}
    });
    let mut hierarchy = engine(json!([outer]))?;
    // At project frame 7, the shot, outer, inner and leaf clocks read 6, 4, 3 and 2.
    // The opposite quarter turns cancel. In world space the leaf's local (u,v)
    // is (17.75 + 1.5u, 18 + 1.5v), and its own fade is halfway through.
    let mut flat = engine(json!([{"content":artwork,"motion":[
        {"frame":0,"x":17.75,"y":18,"scale":1.5,"opacity":0.5}
    ]}]))?;
    let expected = flat.render(7)?.rgba;
    assert!(expected.chunks_exact(4).any(|pixel| pixel[0] > 180));
    assert_eq!(hierarchy.render(7)?.rgba, expected);
    let empty = [0, 0, 0, 255].repeat(40 * 32);
    assert_eq!(hierarchy.render(4)?.rgba.as_slice(), empty); // Child's entrance is at 5.
    assert_eq!(hierarchy.render(10)?.rgba.as_slice(), empty); // Its exclusive end is 10.
    assert_eq!(hierarchy.render(7)?.rgba, expected);
    Ok(())
}

#[test]
fn parent_and_child_translations_are_sampled_on_the_same_shutter_clock() -> Result<()> {
    let artwork = json!({"type":"path","d":"M2 4H4V9H2Z","fill":[255,255,255,255]});
    let motion = json!([{"frame":0},{"frame":8,"x":16}]);
    let hierarchy = json!([{
        "motion":motion,
        "content":{"type":"transform_group","children":[{"content":artwork,"motion":motion}]}
    }]);
    let flat = json!([{"content":artwork,"motion":[{"frame":0},{"frame":8,"x":32}]}]);
    let exposed = |graphics: Value| -> Result<_> {
        let mut body = edit(graphics);
        body["exposure"] = json!({"samples":2,"angle_degrees":360});
        Ok(Engine::new(serde_json::from_value(body)?)?.render(3)?.rgba)
    };
    let composed = exposed(hierarchy)?;
    assert_ne!(composed, engine(flat.clone())?.render(3)?.rgba);
    assert_eq!(composed, exposed(flat)?);
    Ok(())
}

#[test]
fn a_turned_square_inherits_its_parents_scale_in_parent_axes() -> Result<()> {
    let square = json!({"content":{"type":"path","d":"M-5 -5H5V5H-5Z","fill":[255,255,255,255]},
                        "transform":{"rotation_degrees":45}});
    let mut scene = engine(json!([{
        "position":[20,16], "channels":{"scale_x":[{"frame":0,"value":1},{"frame":4,"value":2}]},
        "content":{"type":"transform_group","children":[square]}
    }]))?;
    let stretched = scene.render(5)?;
    let identity = scene.render(1)?;
    assert_ne!(identity.rgba, stretched.rgba);
    assert_eq!(identity.trace.graphics.graphic_uploads, 0);
    let again = scene.render(5)?;
    assert_eq!(again.trace.graphics.graphic_uploads, 0);
    assert_eq!(again.rgba, stretched.rgba);

    // Test pixel centers in the interior and exterior analytically. Leave a
    // texel around the outline for the renderer's antialiasing and sampling.
    for y in 0..32 {
        for x in 0..40 {
            let dx = (x as f64 + 0.5 - 20.) / 2.;
            let dy = y as f64 + 0.5 - 16.;
            let edge = ((dx + dy).abs().max((dy - dx).abs())) / 2_f64.sqrt();
            let red = stretched.rgba[4 * (y * 40 + x)];
            if edge < 4. || edge > 6. {
                assert_eq!(red, if edge < 4. { 255 } else { 0 }, "pixel ({x},{y})");
            }
        }
    }
    Ok(())
}

#[test]
fn unwrapped_rotation_keeps_a_shadow_attached_through_seeks_and_full_turns() -> Result<()> {
    let shader = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/gaussian-blur.wgsl");
    let strip = json!({"position":[20,16],"content":{"type":"path","d":"M-12 -2H12V2H-12Z",
                       "fill":[255,255,255,255]},"underlay":{"shader":shader,"sigma_px":2,
                       "offset":[3,0],"rgba":[255,0,0,255]}});
    let mut animated = strip.clone();
    animated["channels"] = json!({"rotation":[{"frame":0,"value":0},{"frame":8,"value":720}]});
    let mut spinning = engine(json!([animated]))?;
    let mut turned = strip.clone();
    turned["transform"] = json!({"rotation_degrees":90});
    let vertical = engine(json!([turned]))?.render(1)?.rgba;
    let horizontal = engine(json!([strip]))?.render(1)?.rgba;
    assert_ne!(vertical, horizontal);
    // At the top of the vertical strip, a cast shadow reaches a pixel well
    // beyond the bounds of either horizontal endpoint.
    let behind = &vertical[(6 * 40 + 24) * 4..][..4];
    assert!(behind[0] > 0 && behind[1] == 0, "shadow pixel {behind:?}");
    for (frame, expected) in [
        (9, &horizontal),
        (2, &vertical),
        (5, &horizontal),
        (6, &vertical),
    ] {
        assert_eq!(&spinning.render(frame)?.rgba, expected, "frame {frame}");
    }
    Ok(())
}

#[test]
fn a_rotated_parent_preserves_backdrop_access_and_paint_order() -> Result<()> {
    let shader = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/gaussian-blur.wgsl");
    let backdrop = |d: &str| {
        json!({
            "content":{"type":"backdrop","filter":{"shader":shader,"sigma_px":2}},
            "mask":{"content":{"type":"path","d":d}}
        })
    };
    let white = json!({"content":{"type":"path","d":"M16 0H40V32H16Z","fill":[255,255,255,255]}});
    let green = json!({"content":{"type":"path","d":"M15 7H17V9H15Z","fill":[0,255,0,255]}});
    let parent = json!({
        "position":[21,4],"transform":{"rotation_degrees":90},
        "motion":[{"frame":0},{"frame":4,"x":4}],
        "content":{"type":"transform_group","children":[backdrop("M0 0H8V12H0Z")]}
    });
    // At shot frame 2 the mask maps to [11,23) x [4,12) and blurs the white edge.
    let flat = backdrop("M11 4H23V12H11Z");
    let mut expected = engine(json!([white, flat, green]))?;
    let mut grouped = engine(json!([white, parent, green]))?;
    let pixels = expected.render(3)?.rgba;
    let pixel = |x: usize, y: usize| &pixels[(40 * y + x) * 4..(40 * y + x + 1) * 4];
    assert!((1..255).contains(&pixel(15, 5)[0]));
    assert_eq!(pixel(15, 7), &[0, 255, 0, 255]); // Higher content stays sharp.
    assert_eq!(grouped.render(3)?.rgba, pixels);

    let isolation = json!({"content":{"type":"group","children":[flat]}});
    let unchanged = engine(json!([white, green]))?.render(3)?.rgba;
    assert_ne!(unchanged, pixels);
    assert_eq!(
        engine(json!([white, isolation, green]))?.render(3)?.rgba,
        unchanged
    );
    Ok(())
}

#[test]
fn a_spatial_parent_does_not_establish_a_knockout_destination() -> Result<()> {
    let tile = json!({"content":{"type":"path","d":"M0 0H8V8H0Z","fill":[255,255,255,255]}});
    let eraser = json!({"blend":"destination_out","content":{"type":"path","d":"M0 0H2V8H0Z"}});
    let positioned =
        json!({"position":[3,0],"content":{"type":"transform_group","children":[eraser]}});
    // A knockout may only erase pixels within an explicit isolated group.
    assert!(engine(json!([tile, positioned])).is_err());
    let isolated = json!({"content":{"type":"group","children":[tile,positioned]}});
    let rgba = engine(json!([isolated]))?.render(1)?.rgba;
    for x in 0..8 {
        let expected = if (3..5).contains(&x) { 0 } else { 255 };
        assert_eq!(rgba[(2 * 40 + x) * 4], expected);
    }
    Ok(())
}

#[test]
fn an_isolated_childs_mask_and_filter_follow_its_parent_in_local_coordinates() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let shader = directory.path().join("ink.wgsl");
    fs::write(
        &shader,
        "fn effect(p:vec2<i32>, color:vec4<f32>, amount:f32) -> vec4<f32> {
        return vec4(color.r * f32(p.x % 2), color.gba);
    }",
    )?;
    let isolated = json!({
        "position":[2,4],
        "mask":{"position":[-1,2],"content":{"type":"path","d":"M1 1H10V6H1Z"}},
        "filter":{"shader":shader,"amount":100},
        "content":{"type":"group","children":[
            {"content":{"type":"path","d":"M0 0H15V13H0Z","fill":[255,100,50,255]}},
            {"position":[5,4],"content":{"type":"path","d":"M0 0H6V6H0Z","fill":[200,255,240,128]}}
        ]}
    });
    let original = engine(json!([isolated]))?.render(1)?.rgba;
    let parent = json!({
        "position":[27,3],"transform":{"rotation_degrees":90},
        "content":{"type":"transform_group","children":[isolated]}
    });
    let rotated = engine(json!([parent]))?.render(1)?.rgba;
    // Rotate the independently rendered original by an exact quarter turn
    // about its top left, then translate it to (27,3).
    let mut expected = [0, 0, 0, 255].repeat(40 * 32);
    for src_y in 0..27 {
        for src_x in 0..29 {
            let src = 4 * (src_y * 40 + src_x);
            let dst = 4 * ((src_x + 3) * 40 + 26 - src_y);
            expected[dst..dst + 4].copy_from_slice(&original[src..src + 4]);
        }
    }
    assert!(expected.chunks_exact(4).any(|px| px[0] == 255));
    assert!(expected.chunks_exact(4).any(|px| px[0] == 0 && px[1] > 0));
    assert_eq!(rotated.as_slice(), expected);
    Ok(())
}

#[test]
fn editing_a_parent_preserves_children_and_rejects_ambiguous_opacity() -> Result<()> {
    let child = json!({
        "name":"Pointer","position":[2,3],
        "content":{"type":"path","d":"M0 0H4V4H0Z","fill":[255,255,255,255]},
        "motion":[{"frame":0},{"frame":6,"y":6}]
    });
    let parent = json!({
        "name":"Panel","motion":[{"frame":0},{"frame":6,"x":12,"opacity":1}],
        "content":{"type":"transform_group","children":[child]}
    });
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("edit.json");
    fs::write(&file, serde_json::to_vec(&edit(json!([parent])))?)?;
    let mut scene = Session::open(&file)?;
    let rest = scene.render(1)?.rgba;
    let original = scene.render(7)?.rgba;
    let revision = scene.revision();
    let end = "/shots/1/graphics/0/motion/1/frame";
    scene.set(revision, end, json!(10))?;
    let retimed = scene.render(7)?.rgba;
    assert_ne!(retimed, original);
    let row = |pixels: &[u8], y: usize| {
        pixels[40 * y * 4..40 * (y + 1) * 4]
            .chunks_exact(4)
            .any(|p| p[0] > 0)
    };
    assert!(row(&retimed, 9) && row(&original, 9));
    assert!(!row(&retimed, 8) && !row(&original, 8));
    assert_eq!(scene.render(1)?.rgba, rest);
    let revision = scene.revision();
    assert!(
        scene
            .set(revision, "/shots/1/graphics/0/motion/1/opacity", json!(0.5))
            .is_err()
    );
    assert_eq!(scene.revision(), revision);
    scene.save(revision)?;
    let saved: Value = serde_json::from_slice(&fs::read(&file)?)?;
    assert_eq!(
        serde_json::from_value::<Graphic>(
            saved["shots"][1]["graphics"][0]["content"]["children"][0].clone()
        )?,
        serde_json::from_value::<Graphic>(child)?
    );
    assert_eq!(Session::open(&file)?.render(7)?.rgba, retimed);
    scene.undo(revision)?;
    assert_eq!(scene.render(7)?.rgba, original);
    Ok(())
}
