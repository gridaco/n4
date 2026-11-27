use anyhow::Result;
use n4_engine::{Engine, Project, session::Session};
use serde_json::{Value, json};
use std::fs;

fn edit(layer: Value) -> Value {
    json!({
        "size":[32,32], "fps":[30,1],
        "shots":[{"duration":1,"background":{"color":[0,0,0]}},
                 {"duration":16,"background":{"color":[0,0,0]},"graphics":[layer]}]
    })
}

fn tile() -> Value {
    json!({"type":"path","d":"M0 0H4V4H0Z","fill":[255,255,255,255]})
}

fn encoded(linear: f64) -> u8 {
    let srgb = if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    };
    (255. * srgb).round() as u8
}

fn assert_tile(rgba: &[u8], left: usize, top: usize, size: usize, opacity: f64) {
    assert_eq!(rgba.len(), 32 * 32 * 4);
    // The source tile has four fully covered texels per axis and transparent
    // neighbors. At 2x scale each border straddles two bilinear samples.
    let scale = size as f64 / 4.;
    let coverage = |pixel: usize, origin: usize| {
        let local = (pixel as f64 + 0.5 - origin as f64) / scale;
        (local + 0.5).clamp(0., 1.) * (4.5 - local).clamp(0., 1.)
    };
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        let (x, y) = (index % 32, index / 32);
        let expected = encoded(opacity * coverage(x, left) * coverage(y, top));
        assert!(
            pixel[..3].iter().all(|c| c.abs_diff(expected) <= 1),
            "pixel ({x},{y}) was {pixel:?}, expected gray {expected}"
        );
        assert_eq!(pixel[3], 255);
    }
}

#[test]
fn cubic_position_and_held_scale_compose_with_the_childs_own_fade() -> Result<()> {
    let parent = json!({
        "span":[2,15], "position":[2,2],
        "channels":{
            "y":[{"frame":1,"value":0,"ease":{"cubic":[0,1,0,1]}},{"frame":9,"value":16}],
            "scale":[{"frame":0,"value":1,"ease":"hold"},{"frame":9,"value":2}]
        },
        "content":{"type":"transform_group","children":[{
            "content":tile(), "channels":{"opacity":[{"frame":0,"value":0.25,"ease":"hold"},
                                                    {"frame":3,"value":1}]}
        }]}
    });
    let mut engine = Engine::new(serde_json::from_value(edit(parent))?)?;
    // Local zero at film frame 3. In the cubic above the time coordinate is p³
    // and progress is 1−(1−p)³. One eighth of elapsed time gives p=½, so the
    // tile has travelled 14 of 16 pixels while its scale and opacity still hold.
    let midway = engine.render(5)?.rgba;
    assert_tile(&midway, 2, 16, 4, 0.25);
    assert_tile(&engine.render(12)?.rgba, 2, 18, 8, 1.);
    assert_tile(&engine.render(15)?.rgba, 2, 18, 8, 1.);
    assert_tile(&engine.render(3)?.rgba, 2, 2, 4, 0.25);
    assert_tile(&engine.render(4)?.rgba, 2, 2, 4, 0.25);
    // At the held opacity interval's exact right endpoint the tile is opaque.
    assert_eq!(engine.render(6)?.rgba[(19 * 32 + 3) * 4], 255);
    assert_eq!(
        engine.render(16)?.rgba.as_slice(),
        [0, 0, 0, 255].repeat(32 * 32)
    );
    assert_eq!(engine.render(5)?.rgba, midway);
    Ok(())
}

#[test]
fn exposure_samples_the_cubic_at_each_shutter_time() -> Result<()> {
    let mut project = edit(json!({
        "content":tile(), "channels":{"opacity":[
            {"frame":0,"value":0,"ease":{"cubic":[0,1./3.,0,2./3.]}},
            {"frame":8,"value":1}]}
    }));
    project["exposure"] = json!({"samples":2,"angle_degrees":360});
    let mut engine = Engine::new(serde_json::from_value(project)?)?;
    // Time is p³ and progress is p. At shot frame 1, the two samples are at
    // 1.25 and 1.75. The white tile on black exposes their average linear-light value.
    let light = ((1.25_f64 / 8.).cbrt() + (1.75_f64 / 8.).cbrt()) / 2.;
    let expected = engine.render(2)?.rgba;
    assert_tile(&expected, 0, 0, 4, light);
    engine.render(11)?;
    assert_eq!(engine.render(2)?.rgba, expected);
    Ok(())
}

#[test]
fn group_opacity_fades_the_completed_image_once_at_the_overlap() -> Result<()> {
    let group = json!({
        "channels":{"opacity":[{"frame":0,"value":0},{"frame":4,"value":1}]},
        "content":{"type":"group","children":[
            {"content":tile()},{"position":[2,0],"content":tile()}
        ]}
    });
    let mut engine = Engine::new(serde_json::from_value(edit(group))?)?;
    let frame = engine.render(3)?.rgba;
    for x in 0..8 {
        let pixel = &frame[(32 + x) * 4..(32 + x + 1) * 4];
        let expected = if x < 6 { encoded(0.5) } else { 0 };
        // Either child alone and both stacked are the same half-bright white.
        assert!(
            pixel[..3].iter().all(|c| c.abs_diff(expected) <= 1),
            "group pixel at x={x} was {pixel:?}, expected {expected}"
        );
    }
    Ok(())
}

#[test]
fn local_stroke_width_matches_static_paint_after_seeks_and_an_edit() -> Result<()> {
    let shapes = |path_width: Value, rect_width: Value| {
        json!({"type":"transform_group","children":[
            {"content":{"type":"path","d":"M3 7H28","fill":null,
                "stroke":{"rgba":[255,255,255,255],"width":path_width}}},
            {"content":{"type":"rect","x":8,"y":18,"width":15,"height":8,"fill":null,
                "stroke":{"rgba":[255,255,255,255],"width":rect_width}}}
        ]})
    };
    let keys = json!([
        {"frame":0,"value":0.5,"ease":"hold"},
        {"frame":2,"value":2.5},
        {"frame":6,"value":6.5}
    ]);
    let animated = edit(json!({"span":[2,14],"content":shapes(keys.clone(), keys)}));
    let folder = tempfile::tempdir()?;
    let path = folder.path().join("ink.json");
    fs::write(&path, serde_json::to_vec(&animated)?)?;
    let mut session = Session::open(&path)?;

    // Each oracle shot paints the same geometry with two static pen widths.
    let mut oracle = animated.clone();
    oracle["shots"] = [(0.5, 0.5), (2.5, 2.5), (4.5, 4.5), (6.5, 6.5), (4.5, 6.5)]
        .into_iter()
        .map(|(line, border)| {
            json!({"duration":1,"background":{"color":[0,0,0]},
            "graphics":[{"content":shapes(json!(line), json!(border))}]})
        })
        .collect();
    let mut oracle = Engine::new(serde_json::from_value(oracle)?)?;
    // Local frame zero is project frame 3. Visit the hold, the exact step and
    // the interpolated midpoint in both directions on the same warmed engine.
    for (frame, pose) in [(9, 3), (4, 0), (7, 2), (5, 1), (3, 0), (7, 2)] {
        assert!(
            session.render(frame)?.rgba == oracle.render(pose)?.rgba,
            "frame {frame}"
        );
    }
    let original = session.render(9)?.rgba;
    let value = "/shots/1/graphics/0/content/children/0/content/stroke/width/2/value";
    session.set(session.revision(), value, json!(4.5))?;
    let edited = session.render(9)?.rgba;
    assert!(edited != original);
    assert!(edited == oracle.render(4)?.rgba);
    Ok(())
}

#[test]
fn trimming_a_revised_route_keeps_its_two_local_timing_tracks() -> Result<()> {
    let path = |d: &str| {
        json!({
            "type":"path", "d":d, "fill":null, "stroke":{"rgba":[255,255,255,255],"width":2}
        })
    };
    let mut route = path("M4 4 H20 V12"); // lengths 16 + 8
    route["stroke_start"] = json!([{"frame":0,"value":0,"ease":"hold"},{"frame":4,"value":0.25}]);
    route["stroke_end"] =
        json!([{"frame":0,"value":0},{"frame":4,"value":0.75},{"frame":8,"value":1}]);
    let dir = tempfile::tempdir()?;
    let project = dir.path().join("route.json");
    fs::write(
        &project,
        serde_json::to_vec(&edit(json!({"span":[2,14],"content":route})))?,
    )?;
    let mut session = Session::open(&project)?;

    // At local frame 4, draw distances 6..18. Moving the bend changes
    // which leg they fall on while preserving both distances and key times.
    let shots: Vec<_> = ["M4 4 H13", "M10 4 H20 V6", "M10 4 H20 V12", "M10 4 H16 V10"]
        .iter()
        .map(|d| {
            json!({"duration":1,"background":{"color":[0,0,0]},
                               "graphics":[{"content":path(d)}]})
        })
        .collect();
    let mut oracle = Engine::new(serde_json::from_value(
        json!({"size":[32,32],"fps":[30,1],"shots":shots}),
    )?)?;
    // The shot starts at project frame 1 and the layer starts at shot frame 2.
    for (project_frame, pose) in [(11, 2), (5, 0), (7, 1), (5, 0)] {
        assert!(
            session.render(project_frame)?.rgba == oracle.render(pose)?.rgba,
            "frame {project_frame}"
        );
    }
    let geometry = "/shots/1/graphics/0/content/d";
    session.set(session.revision(), geometry, json!("M4 4 H16 V16"))?;
    let revised = session.render(7)?.rgba;
    assert!(revised == oracle.render(3)?.rgba);
    session.save(session.revision())?;
    assert!(Session::open(&project)?.render(7)?.rgba == revised);
    session.undo(session.revision())?;
    assert!(session.render(7)?.rgba == oracle.render(1)?.rgba);
    Ok(())
}

#[test]
fn exposure_samples_the_trimmed_tip_at_each_shutter_time() -> Result<()> {
    let mut project = edit(json!({"content":{
        "type":"path","d":"M4 8H20","fill":null,"stroke":{"rgba":[255,255,255,255],"width":2},
        "stroke_end":[{"frame":0,"value":0},{"frame":1,"value":1}]
    }}));
    project["exposure"] = json!({"samples":2,"angle_degrees":360});
    let mut engine = Engine::new(serde_json::from_value(project)?)?;
    // Local 0.25 and 0.75 put the tip at x=8 and x=16. The middle pixel
    // receives one white and one black sample in linear light.
    engine.render(7)?;
    let frame = engine.render(1)?;
    for (x, level) in [(6, 1.), (12, 0.5), (18, 0.)] {
        let offset = (8 * 32 + x) * 4;
        assert!(
            frame.rgba[offset..offset + 3]
                .iter()
                .all(|v| v.abs_diff(encoded(level)) <= 1),
            "unexpected exposure at x={x}"
        );
    }
    Ok(())
}

#[test]
fn editing_one_timing_curve_survives_save_and_keeps_the_other_keys() -> Result<()> {
    let folder = tempfile::tempdir()?;
    let path = folder.path().join("edit.json");
    let layer = json!({
        "position":[2,2], "content":tile(), "channels":{
            "y":[{"frame":0,"value":0,"ease":{"cubic":[0,0,1,1]}},{"frame":8,"value":16}],
            "opacity":[{"frame":0,"value":0.25},{"frame":4,"value":0.75}]
        }
    });
    let project = edit(layer);
    fs::write(&path, serde_json::to_vec(&project)?)?;
    let accepted = serde_json::to_value(serde_json::from_value::<Project>(project)?)?;
    let mut session = Session::open(&path)?;
    let original = session.render(2)?.rgba;
    assert_tile(&original, 2, 4, 4, 0.375);
    let curve = "/shots/1/graphics/0/channels/y/0/ease/cubic";
    session.set(session.revision(), curve, json!([0, 1, 0, 1]))?;
    let revised = session.render(2)?.rgba;
    assert_tile(&revised, 2, 16, 4, 0.375);
    assert_tile(&session.render(9)?.rgba, 2, 18, 4, 0.75);
    let revision = session.revision();
    assert!(session.set(revision, curve, json!([0, -2, 0, 1])).is_err());
    assert_eq!(session.revision(), revision);
    assert_eq!(session.render(2)?.rgba, revised);
    session.undo(revision)?;
    assert_eq!(session.render(2)?.rgba, original);
    session.redo(session.revision())?;
    session.save(session.revision())?;
    let saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let opacity = "/shots/1/graphics/0/channels/opacity";
    assert_eq!(
        saved.pointer(opacity).unwrap(),
        accepted.pointer(opacity).unwrap()
    );
    assert_eq!(Session::open(&path)?.render(2)?.rgba, revised);
    Ok(())
}

#[test]
fn malformed_or_conflicting_channels_fail_project_admission() -> Result<()> {
    let folder = tempfile::tempdir()?;
    let path = folder.path().join("edit.json");
    let make = |channels| json!({"content":tile(), "channels":channels});
    let key = json!({"frame":0,"value":1});
    let mut mixed = make(json!({"x":[key]}));
    mixed["motion"] = json!([{"frame":0,"x":2}]);
    let parent = json!({"type":"transform_group","children":[{"content":tile()}]});
    let mut parent_fade = make(json!({"opacity":[{"frame":0,"value":0.5}]}));
    parent_fade["content"] = parent;
    let isolated = json!({"type":"group","children":[{"content":tile()}]});
    for layer in [
        make(json!({"x":[key]})),
        json!({"content":isolated, "channels":{"y":[key],"rotation":[key],"opacity":[key]}}),
        json!({"content":isolated, "transform":{"scale_x":0.68}, "channels":{"scale_y":[key]}}),
    ] {
        fs::write(&path, serde_json::to_vec(&edit(layer))?)?;
        Project::load(&path)?;
    }
    for layer in [
        make(json!({"x":[]})),
        make(json!({"x":[key,key]})),
        make(json!({"y":[{"frame":5,"value":0},key]})),
        make(json!({"y":[{"frame":0,"value":0,"ease":{"cubic":[0,1,1]}}]})),
        make(json!({"y":[{"frame":0,"value":0,"ease":{"cubic":[-0.1,1,1,1]}}]})),
        make(json!({"scale":[{"frame":0,"value":0}]})),
        make(json!({"scale_x":[{"frame":0,"value":-2}]})),
        make(json!({"scale":[key],"scale_y":[key]})),
        make(json!({"opacity":[{"frame":0,"value":1.01}]})),
        mixed,
        parent_fade,
        json!({"content":isolated, "transform":{"scale":1.5}, "channels":{"scale_x":[key]}}),
        json!({"content":tile(), "transform":{"rotation_degrees":0}, "channels":{"rotation":[key]}}),
    ] {
        fs::write(&path, serde_json::to_vec(&edit(layer.clone()))?)?;
        assert!(
            Project::load(&path).is_err(),
            "unexpectedly admitted {layer}"
        );
    }
    Ok(())
}
