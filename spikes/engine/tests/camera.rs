use anyhow::Result;
use n4_engine::{CameraSelection, Engine, Project, preview::Filmstrip};
use std::{fs, path::Path};
use tempfile::TempDir;

const ART: &str = r##"<sequence background="#101010" camera="wide">
    <camera id="wide"/>
    <rect x="70" y="8" width="8" height="8" fill="#ff0000"/>
    <camera id="tight" center-x="72"/>
</sequence>"##;
const HUD: &str =
    r##"<sequence><rect x="4" y="2" width="8" height="5" fill="#00ff00"/></sequence>"##;
const CUT: &str = "[0,4) V1 art\n  [0,2) CAM wide\n  [2,4) CAM tight\n[0,8) V2 hud\n[4,8) V1 art\n";

fn setup(art: &str, rows: &str) -> Result<TempDir> {
    let dir = tempfile::tempdir()?;
    let seq = dir.path().join("sequences");
    fs::create_dir(&seq)?;
    fs::write(seq.join("art.xml"), art)?;
    fs::write(seq.join("hud.xml"), HUD)?;
    fs::write(
        dir.path().join("project.proj"),
        format!("project film fps 30/1 size 64x32\n{rows}"),
    )?;
    Ok(dir)
}

fn matte_fixture(shader: &str) -> Result<TempDir> {
    let dir = setup(
        r##"<sequence camera="cam" background="#121212">
            <camera id="cam" projection="perspective" position="0 0 2" fov-y="40"/>
            <scene3d pixels-per-meter="20">
                <plane id="type" position="-1 .5 0">
                    <text font="type.ttf" size="16" width="80" fill="#ffffff"><copy>OO</copy></text>
                </plane>
                <filter source="ink.wgsl"><matte name="coverage" object="type"/></filter>
            </scene3d></sequence>"##,
        "[0,2) V1 art\n",
    )?;
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf"),
        dir.path().join("type.ttf"),
    )?;
    fs::write(dir.path().join("ink.wgsl"), shader)?;
    Ok(dir)
}

#[test]
fn an_unused_matte_binding_and_an_ordinary_filter_share_the_same_shader_file() -> Result<()> {
    let dir =
        matte_fixture("fn effect(p:vec2<i32>, c:vec4<f32>, amount:f32)->vec4<f32> { return c; }")?;
    let declared = Project::load(dir.path())?;
    let mut ordinary = declared.clone();
    ordinary.shots[0].graphics[0]
        .filter
        .as_mut()
        .unwrap()
        .mattes
        .clear();
    let mut engine = Engine::new(ordinary.clone())?;
    let baseline = engine.render(0)?;
    assert_eq!(baseline.trace.graphics.object_matte_passes, 0);
    assert!(baseline.rgba.chunks_exact(4).any(|p| p[0] > 200));

    engine.replace_project(declared)?;
    let ignored_input = engine.render(0)?;
    assert_eq!(baseline.rgba, ignored_input.rgba);

    engine.replace_project(ordinary)?;
    let ordinary_again = engine.render(0)?;
    assert_eq!(ordinary_again.trace.graphics.object_matte_passes, 0);
    assert_eq!(baseline.rgba, ordinary_again.rgba);
    Ok(())
}

#[test]
fn undeclared_matte_helper_leaves_the_accepted_frame_and_revision_intact() -> Result<()> {
    let shader = "fn effect(p:vec2<i32>, c:vec4<f32>, a:f32)->vec4<f32> {
        return vec4<f32>(matte_coverage(p), 0., 0., c.a);
    }";
    let dir = matte_fixture(shader)?;
    let mut next = Project::load(dir.path())?;
    let mut engine = Engine::new(next.clone())?;
    let accepted = engine.render(0)?.rgba;
    next.shots[0].graphics[0].filter.as_mut().unwrap().mattes[0].name = "word".into();
    let error = format!("{:#}", engine.replace_project(next.clone()).unwrap_err());
    assert!(error.contains("matte_coverage"), "{error}");
    assert_eq!(engine.revision(), 0);
    assert_eq!(engine.render(0)?.rgba, accepted);

    let repaired = dir.path().join("word.wgsl");
    fs::write(&repaired, shader.replace("matte_coverage", "matte_word"))?;
    next.shots[0].graphics[0].filter.as_mut().unwrap().shader = repaired;
    engine.replace_project(next)?;
    assert_eq!(engine.revision(), 1);
    assert_eq!(engine.render(0)?.rgba, accepted);
    Ok(())
}

#[test]
fn camera_cuts_belong_to_placements_and_resolve_local_ids() -> Result<()> {
    let dir = setup(ART, CUT)?;
    let project = Project::load(dir.path())?;
    let first = &project.shots[0];
    assert_eq!(first.camera_at(1).unwrap().id(), "wide");
    assert_eq!(first.camera_at(2).unwrap().id(), "tight");
    assert_eq!(project.shots[1].camera_at(2).unwrap().id(), "wide");
    assert_eq!(first.graphics.len(), 1);
    for (xml, rows, message) in [
        (
            ART.replace("id=\"tight\"", "id=\"wide\""),
            CUT.to_owned(),
            "duplicate camera ID",
        ),
        (
            ART.replace("camera=\"wide\"", "camera=\"unknown\""),
            CUT.to_owned(),
            "unknown default camera",
        ),
        (
            ART.to_owned(),
            CUT.replace("CAM tight", "CAM unknown"),
            "unknown camera",
        ),
        (
            ART.to_owned(),
            CUT.replace("[2,4) CAM", "[3,4) CAM"),
            "exactly once",
        ),
        (
            ART.to_owned(),
            CUT.replace("[2,4) CAM", "[1,4) CAM"),
            "exactly once",
        ),
        (
            ART.to_owned(),
            CUT.replace("[2,4) CAM", "[2,3) CAM"),
            "whole placement",
        ),
        (
            ART.to_owned(),
            CUT.replace("V2 hud", "V2 hud\n  [0,8) CAM wide"),
            "CAM must belong to a V1",
        ),
        (
            ART.to_owned(),
            CUT.replace("  [2,4) CAM tight", "  [2,4) V1 art"),
            "both camera cuts and a nested edit",
        ),
        (
            ART.replace("center-x=\"72\"", "zoom=\"0\""),
            CUT.to_owned(),
            "zoom must be positive",
        ),
        (
            ART.replace(
                "center-x=\"72\"",
                "projection=\"perspective\" fov-y=\"180\"",
            ),
            CUT.to_owned(),
            "fov-y",
        ),
    ] {
        let invalid = setup(&xml, &rows)?;
        let err = Project::load(invalid.path()).unwrap_err();
        assert!(format!("{err:#}").contains(message), "{err:#}");
    }
    Ok(())
}

#[test]
fn camera_reveals_offscreen_content_and_holds_the_overlay_in_output_pixels() -> Result<()> {
    let dir = setup(ART, CUT)?;
    let mut project = Project::load(dir.path())?;
    project.exposure = Some(n4_engine::graphics::Exposure {
        samples: 4,
        angle_degrees: 360,
    });
    let mut engine = Engine::new(project)?;
    let pixel = |picture: &n4_engine::Frame, x: usize, y: usize| -> [u8; 4] {
        let i: usize = (y * 64 + x) * 4;
        picture.rgba[i..i + 4].try_into().unwrap()
    };
    let tight = engine.render(2)?;
    let before_cut = engine.render(1)?;
    let repeat = engine.render(6)?;
    assert_eq!(pixel(&tight, 34, 12), [255, 0, 0, 255]);
    assert_eq!(pixel(&before_cut, 34, 12), [16, 16, 16, 255]);
    assert_eq!(repeat.rgba, before_cut.rgba);
    for picture in [&before_cut, &tight, &repeat] {
        assert_eq!(pixel(picture, 7, 4), [0, 255, 0, 255]);
    }
    assert_eq!(
        tight.trace.request.camera_selection,
        CameraSelection::PlacementCut
    );
    assert_eq!(
        repeat.trace.request.camera_selection,
        CameraSelection::DetailDefault
    );
    assert_eq!(tight.rgba, engine.render(2)?.rgba);
    Ok(())
}

#[test]
fn primary_footage_and_filmstrip_use_the_picture_camera_even_without_graphics() -> Result<()> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv");
    let media = format!(
        r#"<sequence source="{}" camera="wide">
        <camera id="offscreen" center-x="2500"/><camera id="wide"/></sequence>"#,
        fixture.display()
    );
    let dir = setup(
        &media,
        "[0,2) V1 art\n[2,4) V1 art\n  [0,2) CAM offscreen\n",
    )?;
    let project = Project::load(dir.path())?;
    let mut project = project;
    project.size = [641, 360];
    let mut plain = project.clone();
    plain.shots[0].camera = None;
    let mut engine = Engine::new(project.clone())?;
    let wide = engine.render(0)?;
    assert_eq!(wide.rgba, Engine::new(plain)?.render(0)?.rgba);
    assert!(wide.rgba.chunks_exact(4).any(|p| p[0] > 16));
    let panned = engine.render(2)?;
    assert!(panned.rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    let strip = Filmstrip::open(project, 64)?;
    let thumb = strip.wait(strip.request(2))?;
    assert_eq!(thumb.source, None);
    assert!(thumb.rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    Ok(())
}
