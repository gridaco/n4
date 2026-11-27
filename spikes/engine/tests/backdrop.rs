use anyhow::Result;
use n4_engine::{Engine, Project, graphics::Exposure, session::Session};
use serde_json::json;
use std::{fs, path::Path};

#[test]
fn material_parameters_follow_the_local_layer_clock_on_random_seek() -> Result<()> {
    let dir = tempfile::tempdir()?;
    fs::create_dir(dir.path().join("sequences"))?;
    fs::write(
        dir.path().join("project.proj"),
        "project test fps 30/1 size 32x32\n[0,10) V1 optics\n",
    )?;
    fs::write(
        dir.path().join("probe.wgsl"),
        r#"
        fn backdrop_pixel(local: vec2f, size: vec2f, screen: vec2f) -> vec4f {
            let p = backdrop_params();
            return vec4f(p.x, p.y, p.z + p.w, 1.0);
        }
    "#,
    )?;
    fs::write(
        dir.path().join("sequences/optics.xml"),
        r##"
        <sequence background="#000000">
          <backdrop span="2 7" source="probe.wgsl" sample-radius="1">
            <param name="mix"><key frame="0" value="0"/><key frame="2" value="1"/></param>
            <param name="tint" value="0.25"/>
            <mask><rect width="32" height="32"/></mask>
          </backdrop>
        </sequence>"##,
    )?;
    let mut engine = Engine::new(Project::load(dir.path())?)?;
    let center = 4 * (16 * 32 + 16);
    for (frame, expected) in [
        (4, [255, 137, 0]),
        (1, [0, 0, 0]),
        (3, [188, 137, 0]),
        (2, [0, 137, 0]),
        (7, [0, 0, 0]),
        (3, [188, 137, 0]),
    ] {
        let picture = engine.render(frame)?;
        let actual = &picture.rgba[center..center + 3];
        assert!(
            actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
            "frame {frame}: {actual:?} expected {expected:?}"
        );
    }
    let mut project = Project::load(dir.path())?;
    project.exposure = Some(Exposure {
        samples: 2,
        angle_degrees: 360,
    });
    // At the start of the trim, the shutter samples the ramp at local .25 and .75.
    // Mean linear red is .25, which encodes as 137 in sRGB.
    let exposed = Engine::new(project)?.render(2)?;
    assert!(exposed.rgba[center].abs_diff(137) <= 1);
    Ok(())
}

#[test]
fn rejected_sampled_and_coverage_shaders_can_be_fixed_and_retried() -> Result<()> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/frost.json");
    let mut project = serde_json::to_value(Project::load(&fixture)?)?;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("edit.json");
    let item = project["graphics"]
        .as_array()
        .unwrap()
        .iter()
        .position(|g| g["content"]["type"] == "backdrop")
        .unwrap();
    let field = |tail: &str| format!("/graphics/{item}/{tail}");
    let sampled = field("content/filter/shader");
    let coverage = field("mask/shader/shader");
    let copy = |value: &serde_json::Value, name: &str| -> Result<()> {
        fs::copy(value.as_str().unwrap(), dir.path().join(name))?;
        Ok(())
    };
    copy(project.pointer(&sampled).unwrap(), "gaussian.wgsl")?;
    copy(project.pointer(&coverage).unwrap(), "coverage.wgsl")?;
    *project.pointer_mut(&sampled).unwrap() = json!("gaussian.wgsl");
    *project.pointer_mut(&coverage).unwrap() = json!("coverage.wgsl");
    fs::write(&file, serde_json::to_vec(&project)?)?;

    let mut session = Session::open(&file)?;
    let original = session.render(96)?.rgba;
    for (pointer, good, new_shader) in [
        (&sampled, "gaussian.wgsl", "edited-sampled.wgsl"),
        (&coverage, "coverage.wgsl", "edited-coverage.wgsl"),
    ] {
        let revision = session.revision();
        fs::write(dir.path().join(new_shader), "incomplete shader")?;
        assert!(session.set(revision, pointer, json!(new_shader)).is_err());
        assert_eq!(session.revision(), revision);
        assert_eq!(session.render(96)?.rgba, original);
        fs::copy(dir.path().join(good), dir.path().join(new_shader))?;
        session.set(revision, pointer, json!(new_shader))?;
        assert_eq!(session.render(96)?.rgba, original);
    }
    let revision = session.revision();
    assert!(
        session
            .set(revision, &field("mask/shader/period_frames"), json!(0))
            .is_err()
    );
    assert!(
        session
            .set(revision, &field("content/filter/sigma_px"), json!(17))
            .is_err()
    );
    assert_eq!(session.revision(), revision);
    session.save(revision)?;
    let disk = fs::read_to_string(&file)?;
    assert!(!disk.contains(&dir.path().display().to_string()));
    Ok(())
}
