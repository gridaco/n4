use anyhow::Result;
use n4_engine::{AuthoringDocument, inspect::Inspection};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn production(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name)
}

#[test]
fn authored_hierarchy_keeps_group_and_property_clocks() -> Result<()> {
    let project = production("examples/signal-atlas");
    let doc = AuthoringDocument::load(&project, None)?;
    let view = doc.inspection()?;
    let outer = view.placement("V1:analysis@64").unwrap();
    assert_eq!(outer.project, [64, 128]);
    assert_eq!(outer.children.len(), 1);
    assert!(outer.definition.is_none());
    let leaf = &outer.children[0];
    assert_eq!(leaf.name, "sounding");
    assert_eq!(leaf.local, [0, 64]);
    assert_eq!(leaf.project, [64, 128]);
    let def = &view.definitions[leaf.definition.as_ref().unwrap()];
    let path = vec!["Sounder".to_owned(), "Instrument".to_owned()];
    let (notification, motion) = def.property(&path, "translate-y")?;
    assert_eq!([notification.start, notification.end.unwrap()], [8, 64]);
    assert_eq!(
        motion
            .keys
            .iter()
            .map(|key| leaf.project[0] + notification.start + key.frame)
            .collect::<Vec<_>>(),
        [72, 84]
    );
    let mut material_path = path;
    material_path.push("Signal lamp".into());
    let (card, opacity) = def.property(&material_path, "opacity")?;
    assert_eq!(leaf.project[0] + card.start + opacity.keys[1].frame, 76);
    assert_eq!(opacity.sample(4, 0.), 1.);
    let translation = motion.sample(4, 0.);
    assert!(translation > 0. && translation < 24.);

    let cli = Command::new(env!("CARGO_BIN_EXE_n4"))
        .arg("inspect")
        .arg(&project)
        .args([
            "--placement",
            &leaf.path,
            "--layer",
            "Sounder",
            "--layer",
            "Instrument",
            "--layer",
            "Signal lamp",
            "--property",
            "opacity",
            "--frame",
            "76",
        ])
        .output()?;
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let response: Value = serde_json::from_slice(&cli.stdout)?;
    assert_eq!(response["keys"][1]["project_frame"], 76);
    assert_eq!(response["sample"]["layer_frame"], 4);
    assert_eq!(response["sample"]["value"], 1.);
    Ok(())
}

#[test]
fn repeated_definitions_keep_the_camera_schedule_owned_by_each_use() -> Result<()> {
    let project = production("examples/signal-atlas");
    let view = AuthoringDocument::load(&project, None)?.inspection()?;
    let uses = view.uses("survey");
    assert_eq!(uses.len(), 2);
    assert_eq!(uses[0].project, [0, 64]);
    assert_eq!(uses[1].project, [128, 192]);
    assert!(uses[0].children.is_empty());
    assert_eq!(
        uses[1]
            .children
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["wide", "detail", "wide"]
    );
    let cut = view.placement("V1:survey@128/CAM:detail@12").unwrap();
    assert_eq!(cut.project, [140, 176]);
    assert_eq!(cut.local, [12, 48]);
    let def = &view.definitions["survey"];
    assert_eq!(def.file, Path::new("sequences/survey.xml"));
    assert_eq!(def.default_camera.as_deref(), Some("wide"));
    assert!(view.placement("V1:survey@0/CAM:detail@12").is_none());
    Ok(())
}

#[test]
fn single_file_fixture_shows_flat_shots_and_the_frames_touched_by_audio() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let entry = temp.path().join("film.json");
    let rate = n4_engine::FrameRate(30000, 1001);
    let boundary = n4_engine::export::sample_boundary(rate, 1)?;
    fs::write(
        &entry,
        serde_json::to_vec(&json!({
            "fps":[rate.0,rate.1],"size":[64,36],
            "shots":[{"duration":4,"name":"Take","source":"take.mov"}],
            "audio":[{"source":"mix.wav","start_sample":boundary-1,"samples":2,"source_in_sample":0}]
        }))?,
    )?;
    let doc = AuthoringDocument::load(&entry, None)?;
    let inspection: Inspection = doc.inspection()?;
    assert_eq!(inspection.name, "film");
    assert_eq!(inspection.placements[0].name, "Take");
    assert_eq!(inspection.placements[1].project, [0, 2]);
    assert_eq!(
        inspection.definitions["shot-1"].source.as_deref(),
        Some(Path::new("take.mov"))
    );
    assert_eq!(
        inspection.definitions["audio-1"].source.as_deref(),
        Some(Path::new("mix.wav"))
    );
    Ok(())
}
