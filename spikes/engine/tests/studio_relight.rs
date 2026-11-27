//! A packaged texture remains resident through relights and reverse seeks.
#![cfg(n4_openusd)]
use anyhow::Result;
use n4_engine::{Engine, Project};
use std::{fs, path::Path};

#[test]
fn packaged_card_can_be_relit_while_its_textures_remain_resident() -> Result<()> {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../spatial/openusd/tests/data/seam.usdz");
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("sequences"))?;
    fs::copy(source, root.path().join("card.usdz"))?;
    fs::write(
        root.path().join("project.proj"),
        "project relight fps 24/1 size 384x384\n[0,3) V1 card\n",
    )?;
    let path = root.path().join("sequences/card.xml");
    let scene = |direction: &str, environment: i32| {
        format!(
            r##"<sequence background="#09111d" camera="view">
  <camera id="view" projection="perspective" position="0.5 0.5 2.7" fov-y="36"/>
  <scene3d pixels-per-meter="100">
    <studio environment-rotation="{environment}" environment-intensity="0.7" shadow-center="0.5 0.5 0" shadow-extent="2" shadow-bias="0.0002" shadow-softness="2"/>
    <distant-light direction="{direction}" intensity="3"/>
    <usd id="card" source="card.usdz" prim="/Card"/>
  </scene3d>
</sequence>"##
        )
    };
    fs::write(&path, scene("-0.9 -0.55 -0.8", 10))?;
    let mut engine = Engine::new(Project::load(root.path())?)?;
    let first = engine.render(0)?;
    assert_eq!(first.trace.graphics.mesh_uploads, 1);
    fs::write(&path, scene("0.9 -0.55 0.8", 110))?;
    engine.replace_project(Project::load(root.path())?)?;
    let second = engine.render(2)?;
    let left = first.rgba.as_chunks::<4>().0;
    let right = second.rgba.as_chunks::<4>().0;
    let background = left[0];
    let mut covered = 0;
    let mut relit = 0;
    let mut unchanged_background = 0;
    for (a, b) in left.iter().zip(right) {
        if a == &background && b == &background {
            unchanged_background += 1;
            continue;
        }
        covered += 1;
        if (0..3).any(|i| a[i].abs_diff(b[i]) > 12) {
            relit += 1;
        }
    }
    assert!(
        covered > 12_000 && unchanged_background > 12_000,
        "asset silhouette and surrounding frame should be visible"
    );
    assert!(
        relit > covered / 3,
        "the two lighting rigs changed {relit} of {covered} covered pixels"
    );
    assert_eq!(second.trace.graphics.mesh_uploads, 0);
    let reverse = engine.render(0)?;
    assert_eq!(reverse.rgba, second.rgba);
    fs::write(&path, scene("-0.9 -0.55 -0.8", 10))?;
    engine.replace_project(Project::load(root.path())?)?;
    let restore = engine.render(0)?;
    assert_eq!(restore.trace.graphics.mesh_uploads, 0);
    assert_eq!(restore.rgba, first.rgba);
    Ok(())
}
