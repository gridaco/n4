use anyhow::{Context, Result};
use n4_engine::{
    Engine, Project,
    graphics::{Content, Filter, Graphic, Mask, MaskContent, NativePath},
};
use serde_json::json;
use std::{fs, path::Path};

// Same cut and font for both authoring routes. The path, mask and expected
// samples are independent of the longer creator demo.
#[test]
fn native_path_edits_preserve_parity_revision_and_unaffected_artwork() -> Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let mut project = Project::load(&fixtures.join("title.json"))?;
    let base = Engine::new(project.clone())?.render(0)?;
    let drawn = "M -4 -2 H 48 V 28 H -4 Z";
    let window = "M 0 0 H 38 V 24 H 0 Z";
    let authored = Graphic {
        name: None,
        content: Content::Path(NativePath {
            d: drawn.into(),
            fill_rule: Default::default(),
            fill: Some([219, 36, 97, 255]),
            stroke_start: None,
            stroke_end: None,
            stroke: None,
        }),
        position: [76, 73],
        blend: Default::default(),
        span: None,
        type_on: None,
        text_anchor: None,
        text_animators: vec![],
        motion: vec![],
        channels: None,
        transform: None,
        wipe: None,
        effects: vec![],
        underlay: None,
        filter: Some(Filter {
            shader: fixtures.join("creator/dry-ink.wgsl"),
            amount: 0,
            mattes: vec![],
        }),
        mask: Some(Mask {
            shader: None,
            position: [3, 1],
            content: MaskContent::Path {
                d: window.into(),
                fill_rule: Default::default(),
            },
        }),
    };
    let encoded = json!({
        "content": {"type": "path", "d": drawn, "fill": [219,36,97,255]},
        "position": [76,73], "filter": {"shader": "dry-ink.wgsl", "amount": 0},
        "mask": {"position": [3,1], "content": {"type": "path", "d": window}}
    });
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("path.json");
    // Give the parsed document a relative filter reference.
    fs::copy(
        fixtures.join("creator/dry-ink.wgsl"),
        dir.path().join("dry-ink.wgsl"),
    )?;
    let mut doc = serde_json::to_value(&project)?;
    doc["graphics"] = json!([encoded]);
    fs::write(&file, serde_json::to_vec(&doc)?)?;
    let mut parsed = Engine::new(Project::load(&file)?)?;
    project.graphics.push(authored);
    let mut engine = Engine::new(project.clone())?;
    let first = engine.render(0)?;
    assert_eq!(first.rgba, parsed.render(0)?.rgba);
    let pixel = |x: usize, y: usize| (y * first.size[0] as usize + x) * 4;
    let inside = pixel(80, 76);
    let outside = pixel(77, 76);
    assert_eq!(&first.rgba[inside..inside + 4], &[219, 36, 97, 255]);
    assert_eq!(
        &first.rgba[outside..outside + 4],
        &base.rgba[outside..outside + 4]
    );

    project.graphics[0].position = [81, 73];
    project.graphics[0].filter.as_mut().unwrap().amount = 75;
    engine.replace_project(project.clone())?;
    let moved = engine.render(0)?;
    assert_ne!(moved.rgba, first.rgba);
    assert_eq!(moved.trace.graphics.graphic_uploads, 0);
    assert_eq!(moved.trace.graphics.raster_paints, 0);
    assert!(moved.trace.reused_decode);

    if let Content::Path(path) = &mut project.graphics[0].content {
        path.d = "M -4 -2 H 12 V 28 H -4 Z".into();
    }
    engine.replace_project(project.clone())?;
    let revised = engine.render(0)?;
    assert_ne!(revised.rgba, moved.rgba);
    assert_eq!(revised.trace.graphics.graphic_uploads, 1);
    assert_eq!(engine.render(12)?.trace.request.timeline_frame, 12);
    assert_eq!(engine.render(0)?.rgba, revised.rgba);

    let mask = project.graphics[0].mask.as_mut().context("missing mask")?;
    mask.content = MaskContent::Path {
        d: "M wrong".into(),
        fill_rule: Default::default(),
    };
    assert!(engine.replace_project(project.clone()).is_err());
    project.graphics[0].mask.as_mut().unwrap().content = MaskContent::Svg {
        source: dir.path().join("missing-mask.svg"),
    };
    assert!(engine.replace_project(project).is_err());
    let preserved = engine.render(0)?;
    assert_eq!(preserved.trace.revision, revised.trace.revision);
    assert_eq!(preserved.rgba, revised.rgba);
    Ok(())
}

#[test]
fn a_session_pins_and_refreshes_an_svg_used_only_as_a_mask() -> Result<()> {
    use n4_engine::session::Session;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let mut doc = serde_json::to_value(Project::load(&fixtures.join("title.json"))?)?;
    doc["graphics"] = json!([{
        "content": {"type":"path", "d":"M0 0H16V16H0Z", "fill":[190,40,30,255]},
        "mask": {"content":{"type":"svg", "source":"window.svg"}}
    }]);
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("edit.json");
    fs::write(&project, serde_json::to_vec(&doc)?)?;
    let mask = temp.path().join("window.svg");
    let svg = r##"<svg width="16" height="16"><path fill="#ffffff" d="M0 0H16V16H0Z"/></svg>"##;
    fs::write(&mask, svg)?;
    let mut session = Session::open(&project)?;
    // Interrupt an external save before the editor has shown its first frame.
    fs::write(&mask, "<svg")?;
    assert!(session.refresh_assets(0).is_err());
    assert_eq!(session.revision(), 0);
    let original = session.render(0)?;
    assert_eq!(&original.rgba[..4], &[190, 40, 30, 255]);
    fs::write(&mask, svg.replace("M0 0H16V16H0Z", "M8 0H16V16H8Z"))?;
    session.refresh_assets(0)?;
    let refreshed = session.render(0)?;
    assert_eq!(refreshed.trace.revision, 1);
    assert_ne!(&refreshed.rgba[..4], &original.rgba[..4]);
    assert_eq!(&refreshed.rgba[40..44], &original.rgba[40..44]);
    Ok(())
}
