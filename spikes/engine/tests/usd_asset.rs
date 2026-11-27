//! Production asset regression using the optional native importer.
#![cfg(n4_openusd)]
use n4_engine::{Engine, Project};
use nothing_openusd::load;
use std::fs;
use std::path::Path;

#[test]
fn authored_face_subsets_keep_their_enamel_and_normals() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../spatial/openusd/tests/data/enamels.usda");
    let mesh = load(&path, "/Swatch").unwrap();
    assert_eq!(mesh.indices.len(), 9);
    assert_eq!(mesh.materials.len(), 3);
    let mut seen = [false; 3];
    for i in &mesh.indices {
        let v = &mesh.vertices[*i as usize];
        seen[v.material as usize] = true;
        assert!((v.normal.iter().map(|s| s * s).sum::<f32>() - 1.).abs() < 0.00001);
    }
    assert_eq!(seen, [true; 3]);
    let cobalt = mesh.materials.iter().find(|m| m.diffuse[2] > 0.9).unwrap();
    assert_eq!(cobalt.roughness, 0.33);
    assert_eq!(cobalt.clearcoat, 0.42);
    assert!(cobalt.use_specular_workflow);
}

#[test]
fn prepared_mesh_survives_source_removal_and_a_failed_edit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let seq = root.join("sequences");
    fs::create_dir(&seq)?;
    fs::write(
        root.join("project.proj"),
        "project mesh fps 24/1 size 64x64\n[0,2) V1 card\n",
    )?;
    let sequence = r##"<sequence background="#ffffff" camera="main">
        <camera id="main" projection="perspective" position="0 0 3" fov-y="45"/>
        <scene3d pixels-per-meter="20"><usd source="card.usda" prim="/Card"/></scene3d>
    </sequence>"##;
    let source = root.join("card.usda");
    fs::write(seq.join("card.xml"), sequence)?;
    fs::write(
        &source,
        r#"#usda 1.0
(
    metersPerUnit = 1
    upAxis = "Y"
)
def Mesh "Card" {
    uniform token subdivisionScheme = "none"
    point3f[] points = [(-1,-1,0), (1,-1,0), (0,1,0)]
    int[] faceVertexCounts = [3]
    int[] faceVertexIndices = [0,1,2]
}
"#,
    )?;
    let project = Project::load(root)?;
    let preview = n4_engine::preview::Preview::open(project.clone())?;
    let mut engine = Engine::new(project)?;
    fs::rename(&source, root.join("retired.usda"))?;

    let first = engine.render(0)?;
    let cold = preview.wait(preview.cold_request(0))?;
    assert_eq!(cold.rgba, first.rgba);
    assert_eq!(first.trace.graphics.mesh_uploads, 1);
    assert_eq!(&first.rgba[..4], &[255; 4]);
    let center = (32 * 64 + 32) * 4;
    assert_ne!(&first.rgba[center..center + 4], &[255; 4]);

    fs::write(
        seq.join("card.xml"),
        sequence
            .replace("card.usda", "retired.usda")
            .replace("/Card", "/Missing"),
    )?;
    let failure = engine.replace_project(Project::load(root)?).unwrap_err();
    assert!(
        format!("{failure:#}").contains("prim does not exist"),
        "{failure:#}"
    );
    assert_eq!(engine.revision(), first.trace.revision);
    let second = engine.render(1)?;
    assert_eq!(second.trace.graphics.mesh_uploads, 0);
    assert_eq!(first.rgba, second.rgba);
    Ok(())
}

#[test]
fn blender_recorder_preserves_assembly_hinge_and_material_slots() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/field-unit/assets/field.usdc");
    let mesh = load(&path, "/World/Recorder").unwrap();
    let hinge = mesh
        .parts
        .iter()
        .position(|p| p.path == "/World/Recorder/Hinge")
        .unwrap();
    let pivot = mesh.parts[hinge].pivot;
    assert!(
        (pivot[1] - 0.041).abs() < 1e-5 && (pivot[2] + 0.059).abs() < 1e-5,
        "{pivot:?}"
    );
    assert!(mesh.parts.iter().any(|p| p.parent == Some(hinge)));
    for slot in ["/World/_materials/Nameplate", "/World/_materials/Display"] {
        assert!(mesh.materials.iter().any(|m| m.slot == slot), "{slot}");
    }
    assert_eq!(
        mesh.parts.iter().map(|p| p.indices.len()).sum::<usize>(),
        mesh.indices.len()
    );
}
