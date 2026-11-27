//! A path-traced scene is a rasterized Draft until its Final exists, and its Final depends only
//! on what the scene itself evaluates to.
#![cfg(n4_openusd)]
use anyhow::Result;
use n4_engine::{Engine, Project};
use std::{fs, path::Path};

fn project(root: &Path, screen: &str, caption: &str, key_x: f32) -> Result<Project> {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/field-unit/assets");
    fs::create_dir_all(root.join("sequences"))?;
    fs::create_dir_all(root.join("assets"))?;
    for name in ["field.usdc", "N4GridSans.ttf"] {
        if !root.join("assets").join(name).exists() {
            fs::copy(example.join(name), root.join("assets").join(name))?;
        }
    }
    fs::write(
        root.join("project.proj"),
        "project traced fps 24/1 size 160x90\n[0,2) V1 shot\n[0,2) V2 caption\n",
    )?;
    fs::write(
        root.join("sequences/shot.xml"),
        format!(
            r##"<sequence background="#0e1011" camera="close">
  <camera id="close" projection="perspective" position="0.15 0.118 0.215" rotation="-14.9 34.3 0" fov-y="27"/>
  <scene3d pixels-per-meter="3200">
    <studio shadow-center="0 0.061 0" shadow-extent="0.39" shadow-bias="0.0003" shadow-softness="4"/>
    <path-trace samples="4" bounces="2" denoise="false"/>
    <ambient-light intensity="0.05"/>
    <rect-light center="{key_x} 0.52 0.34" aim="0 0.05 0" size="0.5 0.3" intensity="11" color="#fff0dc"/>
    <usd id="ground" source="assets/field.usdc" prim="/World/Studio"/>
    <usd id="recorder" source="assets/field.usdc" prim="/World/Recorder">
      <part prim="/World/Recorder/Hinge" rotation-x="30"/>
      <surface slot="/World/_materials/Display" role="emissive" glow="4" size="64 36">
        <rect width="64" height="36" fill="{screen}"/>
      </surface>
    </usd>
  </scene3d>
</sequence>"##
        ),
    )?;
    fs::write(
        root.join("sequences/caption.xml"),
        format!(
            r##"<sequence><text name="Caption" position="6 70" font="assets/N4GridSans.ttf" size="12" width="150" fill="#ffffff"><copy>{caption}</copy></text></sequence>"##
        ),
    )?;
    Project::load(root)
}

#[test]
fn a_scene_without_its_final_is_drawn_and_counted_as_draft() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut engine = Engine::new(project(root.path(), "#20c0ff", "ONE", -0.36)?)?;
    let draft = engine.render(0)?;
    assert_eq!(draft.trace.graphics.path_traced_draft, 1);
    assert_eq!(draft.trace.graphics.path_traced_final, 0);
    // The rasterizer lit the product from the rect light: the frame is not just its background.
    let background = &draft.rgba[..4];
    assert!(
        draft
            .rgba
            .chunks_exact(4)
            .filter(|p| *p != background)
            .count()
            > 2_000
    );
    if let Some(reason) = Engine::path_trace_unavailable() {
        let Err(error) = engine.render_final(0) else {
            panic!("a Final cannot be produced without a path tracer")
        };
        assert!(format!("{error:#}").contains(reason), "{error:#}");
    }
    Ok(())
}

#[test]
fn a_final_is_traced_once_and_only_scene_edits_trace_again() -> Result<()> {
    if let Some(reason) = Engine::path_trace_unavailable() {
        eprintln!("skipped: {reason}");
        return Ok(());
    }
    // The CPU device needs no GPU program compilation.
    unsafe { std::env::set_var("N4_CYCLES_DEVICE", "CPU") };
    let root = tempfile::tempdir()?;
    let store = tempfile::tempdir()?;
    let mut engine = Engine::new(project(root.path(), "#20c0ff", "ONE", -0.36)?)?;
    engine.persist_finals(store.path().to_path_buf())?;
    let (first, traced) = engine.render_final(0)?;
    assert_eq!(traced, 1);
    assert_eq!(first.trace.graphics.path_traced_final, 1);
    assert_eq!(first.trace.graphics.path_traced_draft, 0);
    // The stored picture is the Final from now on, for interactive renders too.
    let again = engine.render(0)?;
    assert_eq!(again.rgba, first.rgba);
    assert_eq!(engine.render_final(0)?.1, 0);

    // A caption above the scene is not part of it.
    engine.replace_project(project(root.path(), "#20c0ff", "TWO", -0.36)?)?;
    let (captioned, traced) = engine.render_final(0)?;
    assert_eq!(traced, 0, "an edit outside the scene reuses its Final");
    assert_ne!(captioned.rgba, first.rgba, "the caption itself changed");

    // The screen's artwork and a light are part of it.
    engine.replace_project(project(root.path(), "#ff3020", "TWO", -0.36)?)?;
    let (recolored, traced) = engine.render_final(0)?;
    assert_eq!(traced, 1, "new screen artwork is a new picture");
    let differing = recolored
        .rgba
        .chunks_exact(4)
        .zip(captioned.rgba.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 24))
        .count();
    assert!(
        differing > 200,
        "the screen changed colour in {differing} pixels"
    );
    engine.replace_project(project(root.path(), "#ff3020", "TWO", 0.36)?)?;
    assert_eq!(
        engine.render_final(0)?.1,
        1,
        "a moved light is a new picture"
    );

    // A second engine over the same store finds the pictures without tracing.
    let mut reopened = Engine::new(project(root.path(), "#20c0ff", "ONE", -0.36)?)?;
    reopened.persist_finals(store.path().to_path_buf())?;
    let (found, traced) = reopened.render_final(0)?;
    assert_eq!(traced, 0);
    assert_eq!(found.rgba, first.rgba);
    Ok(())
}
