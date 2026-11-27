use anyhow::Result;
use n4_engine::{
    AuthoringDocument, Engine, FrameRate, Project, export::sample_boundary, session::Session,
};
use serde_json::json;
use std::{fs, path::Path};

const HEADER: &str = "project film fps 30000/1001 size 64x36\n";

fn fixtures(dir: &Path) -> Result<()> {
    let details = dir.join("sequences");
    fs::create_dir(&details)?;
    fs::write(
        details.join("intro.json"),
        r#"{"name":"Intro","background":{"color":[0,0,0]}}"#,
    )?;
    fs::write(
        details.join("street.json"),
        r#"{"name":"Street","source":"assets/street.mov","source_in":4}"#,
    )?;
    fs::write(
        details.join("music.json"),
        r#"{"source":"assets/music.wav","source_in_sample":19,"gain":0.75}"#,
    )?;
    Ok(())
}

#[test]
fn pending_detail_validates_every_placement_without_touching_the_saved_cut() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fixtures(root)?;
    fs::write(
        root.join("project.proj"),
        format!("{HEADER}[0,8) V1 intro\n[8,10) V1 intro\n"),
    )?;
    let detail = root.join("sequences/intro.json");
    let saved = fs::read(&detail)?;
    let pending = br#"{"name":"Candidate","background":{"color":[0,0,0]}}"#;
    let doc = AuthoringDocument::load(root, Some(("intro", pending)))?;
    assert_eq!(doc.detail_ids().collect::<Vec<_>>(), ["intro"]);
    assert!(
        doc.project
            .shots
            .iter()
            .all(|s| s.name.as_deref() == Some("Candidate"))
    );
    let longer = br#"{"background":{"color":[0,0,0]},"graphics":[{"span":[0,4],"content":{"type":"image","source":"a.png"}}]}"#;
    assert!(AuthoringDocument::load(root, Some(("intro", longer))).is_err());
    assert!(AuthoringDocument::load(root, Some(("street", pending))).is_err());
    assert_eq!(fs::read(&detail)?, saved);
    Ok(())
}

#[test]
fn nested_v1_and_scoped_audio_resolve_to_the_same_native_edit() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fixtures(root)?;
    let master = root.join("project.proj");
    fs::write(
        &master,
        format!(
            "{HEADER}# Opening then street scene\n\
[0,3) V1 intro\n[3,10) V1 sequence\n  [0,2) V1 intro\n  [1,5) A1 music\n  [2,7) V1 inner\n    [0,5) V1 street\n\
[10,12) V1 intro # Same artwork, another placement\n"
        ),
    )?;
    let rate = FrameRate(30000, 1001);
    let a = sample_boundary(rate, 4)?;
    let b = sample_boundary(rate, 8)?;
    let legacy = root.join("whole.json");
    let background = json!({"color":[0,0,0]});
    fs::write(
        &legacy,
        serde_json::to_vec(&json!({
            "size":[64,36],"fps":[30000,1001],"shots":[
                {"name":"Intro","duration":3,"background":background},
                {"name":"Intro","duration":2,"background":background},
                {"name":"Street","duration":5,"source":"assets/street.mov","source_in":4},
                {"name":"Intro","duration":2,"background":background}
            ],"audio":[{"source":"assets/music.wav","source_in_sample":19,"gain":0.75,"start_sample":a,"samples":b-a}]
        }))?,
    )?;
    assert_eq!(Project::load(&master)?, Project::load(&legacy)?);
    assert_eq!(Project::load(root)?, Project::load(&master)?);
    let original = fs::read(&master)?;
    let session = Session::open(&master)
        .err()
        .expect("split session must decline before a save");
    assert!(session.to_string().contains("edit the .proj master"));
    assert!(
        Session::open(root)
            .err()
            .unwrap()
            .to_string()
            .contains("edit the .proj master")
    );
    assert_eq!(fs::read(master)?, original);
    fs::remove_file(root.join("project.proj"))?;
    let failure = Project::load(root).unwrap_err();
    assert!(failure.to_string().contains("project.proj"));
    Ok(())
}

#[test]
fn rejects_unrepresentable_edits_and_details_that_shadow_master_timing() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fixtures(root)?;
    fs::write(root.join("sequences/overlay.json"), r#"{"graphics":[]}"#)?;
    let master = root.join("project.proj");
    for (rows, cause) in [
        ("[0,4) V1 intro\n[3,6) V1 street", "gaps or overlaps"),
        ("[0,4) V1 part\n  [0,2) V1 intro", "does not fill"),
        ("[0,3) V1 part\n  [0,4) V1 intro", "exceeds parent duration"),
        (
            "[0,4) V1 part\n    [0,4) V1 intro",
            "unexpected indentation",
        ),
        (
            "[0,3) V1 intro\n[0,1) A1 music\n  [0,1) V1 street",
            "only V1",
        ),
        (
            "[0,3) V1 intro\n[0,2) A1 music\n[1,3) A1 music",
            "overlapping A1",
        ),
        ("[0,3) V1 intro\n[1,4) A1 music", "beyond the picture"),
        ("[0,3) V1 film", "conflicting definition"),
        ("[0,3) V1 intro\n[0,3) V3 street", "unsupported track"),
        (
            "[0,7) V1 intro\n[1,5) V2 overlay\n[3,6) V2 overlay",
            "overlapping V2 placements on lines 3 and 4",
        ),
        (
            "[0,3) V1 intro\n[2,5) V2 overlay",
            "V2 extends beyond the picture",
        ),
        ("[0,3) V1 intro\n[1,3) V2 intro", "conflicting definition"),
        ("[0,3) V1 intro\n[1,3) V2 street", "unknown field"),
        ("[0,3) V1 ../outside", "invalid definition ID"),
        (
            "[0,4) V1 intro\n[2,3) A1 music\n[1,2) A1 music",
            "time-first",
        ),
        ("[0,3) V1 missing", "missing.json"),
    ] {
        fs::write(&master, format!("{HEADER}{rows}\n"))?;
        let error = Project::load(&master).unwrap_err();
        assert!(format!("{error:#}").contains(cause), "{error:#}");
    }
    for (name, payload, track, cause) in [
        (
            "intro",
            r#"{"duration":7}"#,
            "V1",
            "duration belongs to the master",
        ),
        ("intro", r#"{"shots":[]}"#, "V1", "unknown field"),
        (
            "music",
            r#"{"start_sample":6}"#,
            "A1",
            "start_sample belongs to the master",
        ),
        (
            "music",
            r#"{"samples":3}"#,
            "A1",
            "samples belongs to the master",
        ),
    ] {
        fs::write(root.join("sequences").join(format!("{name}.json")), payload)?;
        fs::write(&master, format!("{HEADER}[0,3) {track} {name}\n"))?;
        let error = Project::load(&master).unwrap_err();
        assert!(format!("{error:#}").contains(cause), "{error:#}");
    }
    Ok(())
}

#[test]
fn nested_graphics_details_share_the_native_index_and_validate_each_placement() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fixtures(root)?;
    let detail = root.join("sequences/band.json");
    let layer = json!({"name":"stripe", "content":{"type":"path",
        "d":"M0 0H8V4H0Z", "fill":[100,20,10,255]}});
    let document = json!({"graphics":[layer]});
    fs::write(&detail, serde_json::to_vec(&document)?)?;
    fs::write(
        root.join("project.proj"),
        format!(
            "{HEADER}[0,3) V1 intro\n[1,7) V2 band\n[3,9) V1 chapter\n  [0,6) V1 intro\n  [2,4) V2 band\n"
        ),
    )?;
    let accepted = AuthoringDocument::load(root, None)?;
    assert_eq!(accepted.detail_ids().collect::<Vec<_>>(), ["band", "intro"]);
    assert_eq!(
        accepted
            .layer("band", &["stripe".to_string()])?
            .name
            .as_deref(),
        Some("stripe")
    );
    let extents: Vec<_> = accepted
        .project
        .overlays
        .iter()
        .map(|p| (p.start, p.duration))
        .collect();
    assert_eq!(extents, [(5, 2), (1, 6)], "inner V2 paints before outer V2");
    let mut candidate = document.clone();
    candidate["graphics"][0]["span"] = json!([0, 3]);
    let bytes = serde_json::to_vec(&candidate)?;
    assert!(AuthoringDocument::load(root, Some(("band", &bytes))).is_err());
    candidate["graphics"][0]["span"] = json!([0, 2]);
    candidate["graphics"][0]["name"] = json!("wide stripe");
    let bytes = serde_json::to_vec(&candidate)?;
    let changed = AuthoringDocument::load(root, Some(("band", &bytes)))?;
    assert!(
        changed
            .project
            .overlays
            .iter()
            .all(|p| p.graphics[0].name.as_deref() == Some("wide stripe"))
    );
    assert_eq!(fs::read(detail)?, serde_json::to_vec(&document)?);
    Ok(())
}

#[test]
fn trimmed_overlay_video_uses_placement_time_for_decode_and_paint() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fixtures(root)?;
    let video = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    fs::copy(&video, root.join("plate.mkv"))?;
    fs::write(
        root.join("sequences/inset.xml"),
        r#"<sequence><video name="insert" source="plate.mkv" size="64 36"
           start="1" duration="3" source-in="17"/></sequence>"#,
    )?;
    fs::write(
        root.join("project.proj"),
        "project edit fps 30/1 size 64x36\n[0,3) V1 intro\n[3,12) V1 chapter\n  [0,4) V1 intro\n  [2,7) V2 inset\n  [4,9) V1 intro\n",
    )?;
    let mut engine = Engine::new(Project::load(root)?)?;
    let direct: Project = serde_json::from_value(json!({"size":[64,36],"fps":[30,1],
        "shots":[{"source":video,"source_in":17,"duration":3}]}))?;
    let mut expected = Engine::new(direct)?;
    for frame in [8, 6, 7, 9, 5, 4, 10, 6] {
        let picture = engine.render(frame)?;
        let pose: Vec<_> = picture
            .trace
            .request
            .overlays
            .iter()
            .map(|p| (p.detail.as_str(), p.local_frame))
            .collect();
        let expected_placement = (5..10).contains(&frame).then(|| ("inset", frame - 5));
        assert_eq!(pose, expected_placement.into_iter().collect::<Vec<_>>());
        let ordinals: Vec<_> = picture
            .trace
            .request
            .video_layers
            .iter()
            .map(|v| v.source_frame)
            .collect();
        if (6..9).contains(&frame) {
            assert_eq!(ordinals, [17 + frame - 6]);
            assert_eq!(picture.rgba, expected.render(frame - 6)?.rgba);
        } else {
            assert!(ordinals.is_empty());
            assert!(
                picture
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|px| *px == [0, 0, 0, 255])
            );
        }
    }
    Ok(())
}

#[test]
fn xml_definition_has_one_source_and_uses_the_same_placement_clock() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let seq = root.join("sequences");
    fs::create_dir(&seq)?;
    fs::write(
        root.join("project.proj"),
        format!("{HEADER}[0,8) V1 shot\n[2,6) V2 card\n"),
    )?;
    fs::write(
        seq.join("shot.xml"),
        r##"<sequence background="#123456"/>"##,
    )?;
    let xml = br#"<sequence><rect name="Card" width="16" height="12" corner-radius="4 3 2 1"/></sequence>"#;
    fs::write(seq.join("card.xml"), xml)?;
    let doc = AuthoringDocument::load(root, None)?;
    assert_eq!(doc.detail_files()["card"], Path::new("sequences/card.xml"));
    let overlay = &doc.project.overlays[0];
    assert_eq!((overlay.start, overlay.duration), (2, 4));
    let override_xml = br#"<sequence><rect name="Card" width="24" height="12"/></sequence>"#;
    let pending = AuthoringDocument::load(root, Some(("card", override_xml)))?;
    assert_eq!(
        pending.detail_value("card")?["graphics"][0]["content"]["width"].as_f64(),
        Some(24.)
    );
    assert_eq!(fs::read(seq.join("card.xml"))?, xml);
    let media = br#"<sequence name="Interview" source="camera.mp4" source-in="5" />"#;
    let edited = AuthoringDocument::load(root, Some(("shot", media)))?;
    assert_eq!(
        edited.project.shots[0].source,
        Some(root.join("camera.mp4"))
    );
    assert_eq!(edited.project.shots[0].source_in, 5);
    assert_eq!(edited.project.shots[0].duration, 8);
    fs::write(seq.join("card.json"), b"{}")?;
    let ambiguous = Project::load(root).unwrap_err();
    assert!(format!("{ambiguous:#}").contains("ambiguous detail card"));
    Ok(())
}
