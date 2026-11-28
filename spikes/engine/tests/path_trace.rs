//! A path-traced scene is a rasterized Draft until its Final exists, and its Final depends only
//! on what the scene itself evaluates to.
#![cfg(n4_openusd)]
use anyhow::Result;
use n4_engine::{Engine, Project};
use std::{fs, path::Path};

/// Tests that trace have nothing to run in a build without the path tracer.
/// `N4_REQUIRE_CYCLES=1` makes that a failure, for a machine that is meant to have it.
fn tracer() -> bool {
    let Some(reason) = Engine::path_trace_unavailable() else {
        return true;
    };
    assert!(
        std::env::var_os("N4_REQUIRE_CYCLES").is_none(),
        "N4_REQUIRE_CYCLES is set: {reason}"
    );
    eprintln!("skipped: {reason}");
    false
}

fn project(root: &Path, screen: &str, caption: &str, key_x: f32) -> Result<Project> {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/field-unit/assets");
    fs::create_dir_all(root.join("sequences"))?;
    fs::create_dir_all(root.join("assets"))?;
    for name in ["field.usdc", "N4GridSans.ttf"] {
        if !root.join("assets").join(name).exists() {
            fs::copy(example.join(name), root.join("assets").join(name))?;
        }
    }
    // Artwork on the screen comes from a file once the test has written one.
    let artwork = if root.join("assets/card.png").exists() {
        r#"<image source="assets/card.png"/>"#
    } else {
        ""
    };
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
        <rect width="64" height="36" fill="{screen}"/>{artwork}
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
    // The same scene with its only rect light on the other side is lit differently.
    let other = tempfile::tempdir()?;
    let moved = Engine::new(project(other.path(), "#20c0ff", "ONE", 0.36)?)?.render(0)?;
    let relit = moved
        .rgba
        .chunks_exact(4)
        .zip(draft.rgba.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 16))
        .count();
    assert!(
        relit > 1_000,
        "moving the rect light changed {relit} pixels"
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
    if !tracer() {
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

    // Export delivers Finals: it traces what is missing and afterwards nothing is left to trace.
    let movie = root.path().join("film.mp4");
    let report = n4_engine::export::write_mp4(&mut engine, &movie)?;
    assert_eq!(
        engine.render_final(1)?.1,
        0,
        "export traced and kept frame 1"
    );
    let again = root.path().join("again.mp4");
    let repeat = n4_engine::export::write_mp4(&mut engine, &again)?;
    assert_eq!(
        repeat.rgba_sha256, report.rgba_sha256,
        "a second export feeds the encoder the same stored pictures"
    );

    // A second engine over the same store finds the pictures without tracing.
    let mut reopened = Engine::new(project(root.path(), "#20c0ff", "ONE", -0.36)?)?;
    reopened.persist_finals(store.path().to_path_buf())?;
    let (found, traced) = reopened.render_final(0)?;
    assert_eq!(traced, 0);
    assert_eq!(found.rgba, first.rgba);
    Ok(())
}

fn write_card(root: &Path, rgb: [u8; 3]) -> Result<()> {
    let file = fs::File::create(root.join("assets/card.png"))?;
    let mut png = png::Encoder::new(std::io::BufWriter::new(file), 64, 36);
    png.set_color(png::ColorType::Rgb);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(&rgb.repeat(64 * 36))?;
    Ok(())
}

#[test]
fn artwork_replaced_on_disk_under_the_same_name_is_traced_again() -> Result<()> {
    if !tracer() {
        return Ok(());
    }
    unsafe { std::env::set_var("N4_CYCLES_DEVICE", "CPU") };
    let root = tempfile::tempdir()?;
    let store = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("assets"))?;
    write_card(root.path(), [0, 200, 40])?;
    let mut engine = Engine::new(project(root.path(), "#000000", "ONE", -0.36)?)?;
    engine.persist_finals(store.path().to_path_buf())?;
    let (green, traced) = engine.render_final(0)?;
    assert_eq!(traced, 1);
    // The same file, untouched, is the same picture, also for a new engine.
    engine.replace_project(project(root.path(), "#000000", "ONE", -0.36)?)?;
    assert_eq!(engine.render_final(0)?.1, 0);

    // A copy of the project elsewhere names other paths and holds the same content.
    let copy = tempfile::tempdir()?;
    fs::create_dir_all(copy.path().join("assets"))?;
    fs::copy(
        root.path().join("assets/card.png"),
        copy.path().join("assets/card.png"),
    )?;
    let mut copied = Engine::new(project(copy.path(), "#000000", "ONE", -0.36)?)?;
    copied.persist_finals(store.path().to_path_buf())?;
    let (same, traced) = copied.render_final(0)?;
    assert_eq!(traced, 0, "a copied project finds its pictures");
    assert_eq!(same.rgba, green.rgba);

    // The scene's description is unchanged; only the file's content differs.
    write_card(root.path(), [230, 20, 20])?;
    let mut reopened = Engine::new(project(root.path(), "#000000", "ONE", -0.36)?)?;
    reopened.persist_finals(store.path().to_path_buf())?;
    let (red, traced) = reopened.render_final(0)?;
    assert_eq!(traced, 1, "replaced artwork is a new picture");
    let differing = red
        .rgba
        .chunks_exact(4)
        .zip(green.rgba.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| a[i].abs_diff(b[i]) > 24))
        .count();
    assert!(
        differing > 200,
        "the screen changed colour in {differing} pixels"
    );
    Ok(())
}

#[test]
fn a_parked_transport_presents_the_draft_and_then_its_final() -> Result<()> {
    if !tracer() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let store = tempfile::tempdir()?;
    unsafe {
        std::env::set_var("N4_CYCLES_DEVICE", "CPU");
        std::env::set_var("N4_FINAL_STORE", store.path());
    }
    let transport =
        n4_engine::transport::Transport::open(project(root.path(), "#20c0ff", "ONE", -0.36)?)?;
    let next = || -> Result<n4_engine::Frame> {
        for _ in 0..3_000 {
            if let Some(frame) = transport.try_next()? {
                return Ok(frame);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        anyhow::bail!("no picture arrived")
    };
    transport.seek(0)?;
    let draft = next()?;
    assert_eq!(draft.trace.graphics.path_traced_draft, 1);
    let finished = next()?;
    assert_eq!(finished.trace.request.timeline_frame, 0);
    assert_eq!(finished.trace.graphics.path_traced_final, 1);
    assert_ne!(finished.rgba, draft.rgba);
    // Returning to the frame shows the Final at once, and nothing further follows.
    transport.seek(1)?;
    transport.seek(0)?;
    let back = next()?;
    assert_eq!(back.rgba, finished.rgba);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(transport.try_next()?.is_none());
    Ok(())
}
