//! CPU-only integration tests for the rigid mesh import profile.
#![cfg(openusd_sdk)]
use nothing_openusd::load;
use std::path::Path;

#[test]
fn a_connected_material_names_the_shader_and_input() {
    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/connected-surface.usda");
    let error = load(&asset, "/Card").unwrap_err().to_string();
    assert!(
        error.contains("/Paint/Surface.inputs:diffuseColor: cannot resolve texture"),
        "{error}"
    );
}

#[test]
fn an_implicit_subdivision_scheme_reports_the_unsupported_surface() {
    let source =
        include_str!("data/hinge.usda").replace("uniform token subdivisionScheme = \"none\"", "");
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("smooth.usda");
    std::fs::write(&asset, source).unwrap();
    let error = load(&asset, "/Rig").unwrap_err().to_string();
    assert!(
        error.contains("/Rig/Hinge/Blade") && error.contains("subdivision surfaces unsupported"),
        "{error}"
    );
}

#[test]
fn packaged_texture_is_resolved_once_for_color_and_data() {
    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/seam.usdz");
    let mesh = load(&asset, "/Card").unwrap();
    assert_eq!(mesh.indices.len(), 6);
    assert_eq!(mesh.images.len(), 1);
    let image = &mesh.images[0];
    assert!(image.source.contains("seam.usdz[quadrants.png]"));
    assert_eq!((image.width, image.height, image.rgba.len()), (2, 2, 16));
    let maps = &mesh.materials[0].maps;
    let color = maps.diffuse.as_ref().unwrap();
    let roughness = maps.roughness.as_ref().unwrap();
    assert_eq!(color.image, roughness.image);
    assert!(color.srgb && !roughness.srgb);
    assert_eq!(roughness.channel, nothing_spatial::TextureChannel::G);
}

#[test]
fn referenced_mesh_is_resolved_and_reopening_reads_the_edited_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root.usda");
    let geometry = dir.path().join("shape.usda");
    std::fs::write(&root, "#usda 1.0\n(\n defaultPrim = \"View\"\n upAxis=\"Z\"\n metersPerUnit=0.01\n)\ndef Xform \"View\" (references=@shape.usda@</Art>) {}\n").unwrap();
    let make = |height: i32| {
        format!(
            "#usda 1.0\ndef Xform \"Art\"\n{{\n def Mesh \"Triangle\"\n {{\n uniform token subdivisionScheme=\"none\"\n point3f[] points=[(0,0,0),(100,0,0),(0,0,{height})]\n int[] faceVertexCounts=[3]\n int[] faceVertexIndices=[0,1,2]\n }}\n}}"
        )
    };
    std::fs::write(&geometry, make(100)).unwrap();
    let first = load(&root, "/View").unwrap();
    assert_eq!(first.vertices[2].position, [0., 1., 0.]);
    std::fs::write(&geometry, make(200)).unwrap();
    let revised = load(&root, "/View").unwrap();
    assert_eq!(revised.vertices[2].position, [0., 2., 0.]);
    let missing = load(&root, "/Missing").unwrap_err().to_string();
    assert!(missing.contains("does not exist"), "{missing}");
}

#[test]
fn a_skinned_mesh_reports_unsupported_deformation() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("rig.usda");
    std::fs::write(
        &asset,
        r#"#usda 1.0
def Xform "Rig" (prepend apiSchemas = ["SkelBindingAPI"]) {
    def Mesh "Body" {}
}
"#,
    )
    .unwrap();
    let error = load(&asset, "/Rig/Body").unwrap_err().to_string();
    assert!(error.contains("skeletal deformation"), "{error}");
}

#[test]
fn indexed_face_varying_uv_seam_and_color_roles_survive_import() {
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join("seam.usda");
    std::fs::write(&stage, include_bytes!("data/seam.usda")).unwrap();
    let rgba = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 127,
    ];
    std::fs::write(
        temp.path().join("quadrants.png"),
        include_bytes!("data/quadrants.png"),
    )
    .unwrap();
    let mesh = load(&stage, "/Card").unwrap();
    let corners: Vec<_> = mesh
        .indices
        .iter()
        .map(|&i| &mesh.vertices[i as usize])
        .collect();
    assert_eq!(corners[0].position, corners[3].position);
    assert_eq!(corners[0].uv, [0., 0.]);
    assert_eq!(corners[3].uv, [0.5, 0.5]);
    assert_eq!(mesh.images.len(), 1);
    assert_eq!(mesh.images[0].rgba, rgba);
    let mat = &mesh.materials[0];
    assert_eq!(mat.slot, "/Print");
    let color = mat.maps.diffuse.as_ref().unwrap();
    let roughness = mat.maps.roughness.as_ref().unwrap();
    assert_eq!(color.image, roughness.image);
    assert!(color.srgb && !roughness.srgb);
    assert_eq!(roughness.channel, nothing_spatial::TextureChannel::G);
    assert_eq!((roughness.scale[1], roughness.bias[1]), (0.5, 0.25));
    let source = include_str!("data/seam.usda");
    for (explicit, implicit, diagnostic) in [
        (
            "inputs:wrapS = \"repeat\"",
            "inputs:wrapS = \"useMetadata\"",
            "wrapS useMetadata",
        ),
        (
            "inputs:sourceColorSpace = \"sRGB\"",
            "inputs:sourceColorSpace = \"auto\"",
            "sourceColorSpace auto",
        ),
    ] {
        std::fs::write(&stage, source.replace(explicit, implicit)).unwrap();
        let error = load(&stage, "/Card").unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{error}");
    }
    std::fs::write(&stage, source).unwrap();
    std::fs::remove_file(temp.path().join("quadrants.png")).unwrap();
    let error = load(&stage, "/Card").unwrap_err().to_string();
    assert!(
        error.contains("/Print/PBR.inputs:diffuseColor") && error.contains("quadrants.png"),
        "{error}"
    );
}

#[test]
fn packed_channels_bind_with_distinct_material_roles() {
    let stage = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/packed-channels.usda");
    let mesh = load(&stage, "/Card").unwrap();
    assert_eq!(mesh.images.len(), 1);
    let maps = &mesh.materials[0].maps;
    let normal = maps.normal.as_ref().unwrap();
    assert!(!normal.srgb);
    assert_eq!(normal.scale[1], 0.5);
    assert_eq!(
        maps.occlusion.as_ref().unwrap().channel,
        nothing_spatial::TextureChannel::R
    );
    assert_eq!(
        maps.metallic.as_ref().unwrap().channel,
        nothing_spatial::TextureChannel::B
    );
}

#[test]
fn rigid_samples_preserve_parent_hinge_space_and_authored_pivot() {
    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/hinge.usda");
    let at = |time| nothing_openusd::load_at(&asset, "/Rig", time).unwrap();
    let a = at(0.);
    let b = at(10.);
    let halfway = at(5.);
    let hinge = &a.parts[1];
    assert_eq!(hinge.path, "/Rig/Hinge");
    assert_eq!(hinge.parent, Some(0));
    assert_eq!(a.parts[2].parent, Some(1));
    assert_eq!(a.parts[2].indices, 0..3);
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(&x, y)| (x - y).abs() < 1e-5);
    assert!(close(hinge.pivot, [0.11, 0.03, -0.22]));
    assert!(close(b.parts[1].pivot, hinge.pivot));
    assert!(close(a.vertices[1].position, [0.2, 0., -0.2]));
    assert!(close(b.vertices[1].position, [0.13, 0., -0.31]));
    assert!(close(
        [
            b.parts[1].frame[0][0],
            b.parts[1].frame[0][1],
            b.parts[1].frame[0][2]
        ],
        [0., 0., -1.]
    ));
    assert!((halfway.parts[1].frame[0][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    let again = at(0.);
    assert_eq!(again.vertices[1].position, a.vertices[1].position);
}
