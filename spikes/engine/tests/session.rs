use anyhow::Result;
use n4_engine::{Project, preview::Preview, session::Session};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write_test_project(directory: &Path) -> Result<std::path::PathBuf> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let project = directory.join("cut.json");
    fs::copy(fixtures.join("title.json"), &project)?;
    fs::create_dir(directory.join("fonts"))?;
    fs::copy(
        fixtures.join("fonts/N4GridSans.ttf"),
        directory.join("fonts/N4GridSans.ttf"),
    )?;
    fs::copy(
        fixtures.join("numbered.mkv"),
        directory.join("numbered.mkv"),
    )?;
    Ok(project)
}

#[test]
fn graphic_edits_reuse_the_accepted_movie_profile_until_asset_refresh() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
    doc["graphics"] = json!([{"position":[1,0],"content":{
        "type":"rect","width":16,"height":16,"fill":[255,0,0,255]
    }}]);
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let original = session.render(0)?.rgba;

    fs::remove_file(tmp.path().join("numbered.mkv"))?;
    session.set(0, "/graphics/0/position/0", json!(32))?;
    let moved = session.render(0)?;
    assert!(moved.trace.reused_decode);
    assert_ne!(moved.rgba, original);
    assert!(session.refresh_assets(1).is_err());
    session.undo(1)?;
    assert_eq!(session.render(0)?.rgba, original);
    Ok(())
}

#[test]
fn curve_handle_edits_preserve_endpoints_and_evaluate_between_local_keys() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = write_test_project(directory.path())?;
    let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let rest = json!([[0, 0], [20, 0], [60, 0], [80, 0]]);
    let bent = json!([[0, 0], [20, -48], [60, -48], [80, 0]]);
    doc["graphics"] = json!([{"span":[4,26], "content":{"type":"group","children":[{
        "position":[50,115], "content":{"type":"path",
            "d":{"keys":[{"frame":0,"points":rest,"ease":"linear"},
                         {"frame":8,"points":bent,"ease":"linear"},
                         {"frame":16,"points":rest}]},
            "fill":null, "stroke":{"width":8,"rgba":[20,90,250,255]}
        }
    }]}}]);
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let opening = session.render(4)?.rgba;
    let ending = session.render(21)?.rgba;
    let middle = session.render(8)?.rgba;

    // Project frame 8 is local frame 4. Each handle is halfway through its bend.
    let geometry = "/graphics/0/content/children/0/content/d";
    let mut oracle = doc.clone();
    *oracle.pointer_mut(geometry).unwrap() = json!("M0 0 C20 -24 60 -24 80 0");
    let still = directory.path().join("halfway.json");
    fs::write(&still, serde_json::to_vec(&oracle)?)?;
    assert_eq!(Session::open(&still)?.render(8)?.rgba, middle);

    session.set(0, &format!("{geometry}/keys/1/points/1"), json!([20, -96]))?;
    let revised = session.render(8)?.rgba;
    assert_ne!(revised, middle);
    assert_eq!(session.render(21)?.rgba, ending);
    assert_eq!(session.render(4)?.rgba, opening);
    assert_eq!(session.render(8)?.rgba, revised);
    assert!(
        session
            .set(1, &format!("{geometry}/keys/1/points"), json!([[0, 0]]))
            .is_err()
    );
    assert_eq!(session.revision(), 1);
    assert_eq!(session.render(8)?.rgba, revised);
    session.save(1)?;
    assert_eq!(Session::open(&path)?.render(8)?.rgba, revised);
    session.undo(1)?;
    assert_eq!(session.render(8)?.rgba, middle);
    Ok(())
}

#[test]
fn retiming_a_display_letter_preserves_the_headline_underneath() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
    doc["title"]["start"] = json!(60);
    let label = |text, size| {
        json!({
            "type":"label", "font":"fonts/N4GridSans.ttf", "size":size, "width":210,
            "rgb":[12, 12, 12], "text":text
        })
    };
    let headline = json!({
        "content":label("Learning together", 27), "position":[100, 85],
        "type_on":[[0,2],[3,5],[6,null]]
    });
    let display = json!({"span":[3,13], "content":label("M", 100), "position":[94,65]});
    doc["graphics"] = json!([{ "span":[10,30], "content":{
        "type":"group", "children":[headline, display]
    }}]);
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let onset = session.render(13)?.rgba;
    let held = session.render(19)?.rgba;
    let exit = session.render(23)?.rgba;

    // Delay the M six frames within its group. The headline and exit keep their cues.
    let entrance = "/graphics/0/content/children/1/span/0";
    session.set(0, entrance, json!(9))?;
    let exposed = session.render(13)?.rgba;
    assert_ne!(exposed, onset);
    assert_eq!(session.render(19)?.rgba, held);
    assert_eq!(session.render(23)?.rgba, exit);
    let mut headline_only = doc.clone();
    headline_only["graphics"][0]["content"]["children"]
        .as_array_mut()
        .unwrap()
        .pop();
    let single = tmp.path().join("headline-only.json");
    fs::write(&single, serde_json::to_vec(&headline_only)?)?;
    assert_eq!(Session::open(&single)?.render(13)?.rgba, exposed);
    assert!(session.set(1, entrance, json!(14)).is_err());
    assert_eq!(session.revision(), 1);
    assert_eq!(session.render(13)?.rgba, exposed);

    session.undo(1)?;
    assert_eq!(session.render(13)?.rgba, onset);
    let copy_path = "/graphics/0/content/children/0/content/text";
    session.set(
        2,
        copy_path,
        json!("Exploring the longer headline together"),
    )?;
    let revised = session.render(19)?.rgba;
    assert_ne!(revised, held);
    session.save(3)?;
    assert_eq!(Session::open(&path)?.render(19)?.rgba, revised);
    Ok(())
}

#[test]
fn source_alpha_underlay_edits_reuse_picture_and_keep_failed_edits_retryable() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let gaussian = include_str!("../fixtures/creator/gaussian-blur.wgsl");
    fs::write(tmp.path().join("alpha.wgsl"), gaussian)?;
    let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
    doc["graphics"] = json!([{
        "position": [80, 80],
        "content": {"type": "path", "d":"M0 0H40V24H0Z", "fill":[226,200,64,255]},
        "underlay": {"shader":"alpha.wgsl", "sigma_px":4, "offset":[8,6], "rgba":[128,0,0,200]}
    }]);
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let first = session.render(0)?.rgba;
    session.set(0, "/graphics/0/underlay/offset", json!([-14, 9]))?;
    let moved = session.render(0)?;
    assert_ne!(moved.rgba, first);
    assert!(moved.trace.reused_decode);
    assert_eq!(moved.trace.graphics.graphic_uploads, 0);
    assert_eq!(moved.trace.graphics.raster_paints, 0);

    let shader_pointer = "/graphics/0/underlay/shader";
    fs::write(tmp.path().join("edit.wgsl"), "invalid WGSL")?;
    assert!(session.set(1, shader_pointer, json!("edit.wgsl")).is_err());
    assert!(
        session
            .set(1, "/graphics/0/underlay/sigma_px", json!(17))
            .is_err()
    );
    let mut adjustment = doc["graphics"][0].clone();
    adjustment["content"] =
        json!({"type":"backdrop","filter":{"shader":"alpha.wgsl","sigma_px":2}});
    adjustment["mask"] = json!({"content":{"type":"path","d":"M0 0H20V20H0Z"}});
    let error = session.set(1, "/graphics/0", adjustment).unwrap_err();
    assert!(error.to_string().contains("source layer or group"));
    assert_eq!(session.revision(), 1);
    assert_eq!(session.render(0)?.rgba, moved.rgba);

    // Changing the sampled program can remove the underlay without touching the caster.
    fs::write(
        tmp.path().join("edit.wgsl"),
        "fn filter_pixel(p: vec2<i32>) -> vec4<f32> { return vec4<f32>(0.0); }",
    )?;
    session.set(1, shader_pointer, json!("edit.wgsl"))?;
    let zero_program = session.render(0)?.rgba;
    assert_ne!(zero_program, moved.rgba);
    session.set(2, "/graphics/0/underlay/rgba/3", json!(0))?;
    assert_eq!(zero_program, session.render(0)?.rgba);
    session.undo(3)?;
    session.undo(4)?;
    assert_eq!(session.render(0)?.rgba, moved.rgba);
    session.save(5)?;
    let saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert_eq!(saved.pointer(shader_pointer), Some(&json!("alpha.wgsl")));
    assert_eq!(Session::open(&path)?.render(0)?.rgba, moved.rgba);
    Ok(())
}

#[test]
fn two_editors_undo_redo_reload_and_interrupted_link() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let mut s = Session::open(&path)?;
    let before = s.render(15)?;
    s.set(0, "/title/width", json!(240))?;
    let revised = s.render(15)?;
    assert_ne!(before.rgba, revised.rgba);
    assert!(revised.trace.reused_decode);
    assert!(s.set(0, "/title/width", json!(150)).is_err());
    s.save(1)?;
    let saved = fs::read(&path)?;
    assert!(!String::from_utf8_lossy(&saved).contains(&tmp.path().display().to_string()));
    s.save(1)?;
    assert_eq!(saved, fs::read(&path)?);
    let mut patched: Value = serde_json::from_slice(&saved)?;
    patched["title"]["start"] = json!(12);
    fs::write(&path, serde_json::to_vec(&patched)?)?;
    s.reload(1)?;
    let patched_frame = s.render(15)?;
    assert!(patched_frame.trace.reused_decode && patched_frame.trace.reused_paragraph);
    assert_ne!(revised.rgba, patched_frame.rgba);
    s.undo(2)?;
    assert_eq!(revised.rgba, s.render(15)?.rgba);
    s.redo(3)?;
    assert_eq!(patched_frame.rgba, s.render(15)?.rgba);
    s.set(4, "/title/start", json!(13))?;
    fs::write(&path, b"{")?;
    assert!(s.reload(5).is_err());
    assert!(s.save(5).is_err());
    patched["title"]["start"] = json!(8);
    fs::write(&path, serde_json::to_vec(&patched)?)?;
    assert!(s.reload(5).is_err());
    assert_eq!(s.render(15)?.trace.request.title_opacity, [2, 6]);
    s.undo(5)?;
    s.reload(6)?;
    assert_eq!(s.revision(), 7);
    fs::copy(
        tmp.path().join("numbered.mkv"),
        tmp.path().join("render-v2.mkv"),
    )?;
    s.relink(7, Path::new("numbered.mkv"), Path::new("render-v2.mkv"))?;
    let a = s.render(15)?;
    let b = s.render(45)?;
    assert!(
        [a.trace.request.source, b.trace.request.source]
            .iter()
            .all(|source| source
                .as_deref()
                .is_some_and(|path| path.ends_with("render-v2.mkv")))
    );
    assert_eq!(
        (a.trace.request.source_frame, b.trace.request.source_frame),
        (Some(25), Some(18))
    );
    fs::write(tmp.path().join("render-incomplete.mkv"), b"partial")?;
    assert!(
        s.relink(
            8,
            Path::new("render-v2.mkv"),
            Path::new("render-incomplete.mkv")
        )
        .is_err()
    );
    assert_eq!(s.render(15)?.rgba, a.rgba);
    s.save(8)?;
    let mut reopened = Session::open(&path)?;
    assert_eq!(reopened.render(45)?.rgba, b.rgba);
    Ok(())
}

#[test]
fn edit_and_reorder_a_graphics_only_film() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("film.json");
    let doc = json!({"size":[24,16],"fps":[30,1],"shots":[
        {"name":"Opening","duration":2,"background":{"color":[255,0,0]}},
        {"name":"Rise","duration":3,"background":{"color":[0,0,0]},"graphics":[{
            "name":"Horizon","content":{"type":"path","fill":null,
                "stroke":{"rgba":[255,255,255,255],"width":4},
                "d":{"keys":[
                    {"frame":0,"ease":"linear","points":[[1,12],[8,12],[16,12],[23,12]]},
                    {"frame":2,"points":[[1,4],[8,4],[16,4],[23,4]]}
                ]}}
        }]}
    ]});
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let pixel = |rgba: &[u8], y: usize| -> [u8; 4] {
        let start = (y * 24 + 12) * 4;
        rgba[start..start + 4].try_into().unwrap()
    };
    let first = session.render(0)?;
    let request = serde_json::to_value(&first.trace.request)?;
    assert_eq!(request["shot_frame"], 0);
    assert!(request.get("source").is_none() && request.get("source_frame").is_none());
    let opening = first.rgba;
    assert_eq!(pixel(&opening, 8), [255, 0, 0, 255]);
    session.set(0, "/shots/1/background/color", json!([0, 0, 255]))?;
    let white = session.render(3)?.rgba;
    assert_eq!(pixel(&white, 8), [255; 4]);
    assert_eq!(pixel(&white, 0), [0, 0, 255, 255]);
    session.set(
        1,
        "/shots/1/graphics/0/content/stroke/rgba",
        json!([255, 255, 0, 255]),
    )?;
    let poses = (2..5)
        .map(|f| Ok(session.render(f)?.rgba))
        .collect::<Result<Vec<_>>>()?;
    for (pose, y) in poses.iter().zip([12, 8, 4]) {
        assert_eq!(pixel(pose, y), [255, 255, 0, 255]);
    }
    session.undo(2)?;
    assert_eq!(session.render(3)?.rgba, white);
    session.redo(3)?;
    session.save(4)?;

    let mut saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let cuts = saved["shots"].as_array_mut().unwrap();
    cuts.swap(0, 1);
    session.set(4, "/shots", json!(cuts))?;
    session.save(5)?;
    let mut reopened = Session::open(&path)?;
    for (frame, pose) in poses.iter().enumerate() {
        assert_eq!(&reopened.render(frame as u32)?.rgba, pose);
    }
    assert_eq!(reopened.render(3)?.rgba, opening);
    Ok(())
}

#[test]
fn relink_a_shot_local_clip_and_reopen_without_a_title() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    fs::copy(
        tmp.path().join("numbered.mkv"),
        tmp.path().join("replacement.mkv"),
    )?;
    let doc = json!({"size":[64,32],"fps":[30,1],"shots":[
        {"duration":5,"name":"Lead","background":{"color":[20,30,40]}},
        {"duration":4,"name":"Inset","background":{"color":[50,60,70]},"graphics":[
            {"span":[1,4],"content":{"type":"group","children":[
                {"content":{"type":"video","source":"numbered.mkv","source_in":7,
                            "start":1,"duration":3,"size":[32,18]}}
            ]}}
        ]}
    ]});
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let before = session.render(6)?;
    assert_eq!(before.trace.request.video_layers[0].source_frame, 7);
    assert!(session.render(5)?.trace.request.video_layers.is_empty());
    let source = Path::new("numbered.mkv");
    let target = Path::new("replacement.mkv");
    session.relink(0, source, target)?;
    let after = session.render(6)?;
    assert!(after.trace.request.video_layers[0].source.ends_with(target));
    assert_eq!(after.rgba, before.rgba);
    session.save(1)?;
    let saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert!(saved.get("title").is_none());
    for shot in saved["shots"].as_array().unwrap() {
        assert!(shot.get("source").is_none() && shot.get("source_in").is_none());
    }
    assert_eq!(
        saved.pointer("/shots/1/graphics/0/content/children/0/content/source"),
        Some(&json!("replacement.mkv"))
    );
    assert_eq!(Session::open(&path)?.render(6)?.rgba, before.rgba);
    session.undo(1)?;
    assert!(
        session.render(6)?.trace.request.video_layers[0]
            .source
            .ends_with(source)
    );
    Ok(())
}

#[test]
fn slip_and_relink_a_nested_video_without_changing_the_base_cut() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
    // The same file plays at two different times and two different sizes.
    doc["graphics"] = json!([
        {"content":{"type":"footage"}},
        {"content":{"type":"group","children":[{
            "position":[16,12],
            "content":{"type":"video","source":"numbered.mkv","start":10,
                       "source_in":41,"duration":20,"size":[160,90]}
        }]}}
    ]);
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let mut session = Session::open(&path)?;
    let first = session.render(15)?;
    let outside = session.render(30)?;
    let read_id = |rgba: &[u8], x: usize, step: usize, y: usize| -> u32 {
        (0..7).fold(0, |id, bit| {
            let red = rgba[(y * 641 + x + bit * step) * 4];
            id | (u32::from(red > 128) << bit)
        })
    };
    let ids = |pixels: &[u8]| (read_id(pixels, 16, 16, 344), read_id(pixels, 20, 4, 98));
    assert_eq!(ids(&first.rgba), (25, 46));

    let pointer = "/graphics/1/content/children/0/content/source_in";
    session.set(0, pointer, json!(53))?;
    let slipped = session.render(15)?;
    assert_eq!(ids(&slipped.rgba), (25, 58));
    assert_eq!(session.render(30)?.rgba, outside.rgba);
    assert!(session.set(1, pointer, json!(80)).is_err());
    assert_eq!(session.revision(), 1);
    assert_eq!(session.render(15)?.rgba, slipped.rgba);
    session.undo(1)?;
    assert_eq!(session.render(15)?.rgba, first.rgba);
    session.redo(2)?;

    fs::copy(tmp.path().join("numbered.mkv"), tmp.path().join("take.mkv"))?;
    session.relink(3, Path::new("numbered.mkv"), Path::new("take.mkv"))?;
    session.save(4)?;
    let saved: Value = serde_json::from_slice(&fs::read(&path)?)?;
    assert_eq!(saved["shots"][0]["source"], "take.mkv");
    assert_eq!(
        saved["graphics"][1]["content"]["children"][0]["content"]["source"],
        "take.mkv"
    );
    assert_eq!(Session::open(&path)?.render(15)?.rgba, slipped.rgba);
    Ok(())
}

#[test]
fn superseding_scrub_requests_only_delivers_the_current_frame() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let preview = Preview::open(Project::load(&path)?)?;
    let tickets: Vec<_> = [15, 40, 0, 59, 30]
        .into_iter()
        .map(|frame| preview.request(frame))
        .collect();
    for ticket in &tickets[..4] {
        assert!(preview.wait(*ticket).is_err());
    }
    let result = preview.wait(tickets[4])?;
    assert_eq!(result.trace.request.timeline_frame, 30);
    assert_eq!(result.trace.request.source_frame, Some(3));
    Ok(())
}

#[test]
fn thumbnail_matches_trim_after_viewport_changes() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let strip = n4_engine::preview::Filmstrip::open(Project::load(&path)?, 160)?;
    let stale = strip.request(10);
    let current = strip.request(30);
    assert!(strip.wait(stale).is_err());
    let thumb = strip.wait(current)?;
    assert_eq!(thumb.size, [160, 90]);
    assert_eq!(thumb.source_frame, 3);
    // The barcode in the source occupies a row of high-contrast patches after downsampling.
    let mut id = 0u32;
    for bit in 0..7 {
        let x = 4 + 4 * bit;
        if thumb.rgba[(86 * 160 + x) * 4] > 128 {
            id |= 1 << bit;
        }
    }
    assert_eq!(id, 3);
    Ok(())
}

#[test]
fn refresh_reloads_same_path_artwork_and_rejects_a_broken_replacement() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let path = write_test_project(tmp.path())?;
    let svg = tmp.path().join("badge.svg");
    let mut doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
    doc["graphics"] = json!([{"position":[0,0],"content":{"type":"svg","source":"badge.svg"}}]);
    fs::write(&path, serde_json::to_vec(&doc)?)?;
    let original = r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><path fill="#ff0000" d="M0 0H32V32H0Z"/></svg>"##;
    fs::write(&svg, original)?;
    let mut s = Session::open(&path)?;
    let initial = s.render(15)?;
    fs::write(&svg, original.replace("#ff0000", "#0000ff"))?;
    s.refresh_assets(0)?;
    let updated = s.render(15)?;
    assert_ne!(&initial.rgba[..4], &updated.rgba[..4]);
    fs::write(&svg, "<svg")?;
    assert!(s.refresh_assets(1).is_err());
    assert_eq!(s.revision(), 1);
    assert_eq!(s.render(15)?.rgba, updated.rgba);
    Ok(())
}
