//! Units and conventions of the bridge, on quantities with known answers. Uses Cycles' CPU device
//! so it needs no GPU program compilation. Skipped when no Cycles bundle is provisioned.
use anyhow::Result;
use nothing_spatial::{
    Image, Material, MaterialMaps, Mesh, Part, TextureChannel, TextureMap, Vertex,
};
use nothing_spatial_cycles::{Artwork, Frame, Recipe, Session, available};

const IDENTITY: [[f32; 4]; 4] = [
    [1., 0., 0., 0.],
    [0., 1., 0., 0.],
    [0., 0., 1., 0.],
    [0., 0., 0., 1.],
];

/// A one-metre square on the floor, facing up, with UVs across it.
fn floor(material: Material, images: Vec<Image>) -> Mesh {
    let mut mesh = Mesh {
        materials: vec![material],
        images,
        ..Default::default()
    };
    for (x, z) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
        mesh.vertices.push(Vertex {
            position: [x, 0., z],
            normal: [0., 1., 0.],
            material: 0,
            uv: [x + 0.5, z + 0.5],
        });
    }
    mesh.indices = vec![0, 2, 1, 0, 3, 2];
    mesh.parts.push(Part {
        path: "/floor".into(),
        parent: None,
        indices: 0..6,
        frame: IDENTITY,
        pivot: [0.; 3],
    });
    mesh
}

fn overhead(s: &mut Session) {
    s.background([0.; 3], false);
    s.camera(
        [0., 2., 0.],
        [1., 0., 0.],
        [0., 0., -1.],
        [0., -1., 0.],
        40.,
    );
}

fn red(f: &Frame, x: usize) -> f32 {
    f.color[(32 * 64 + x) * 4]
}

const RECIPE: Recipe = Recipe {
    samples: 256,
    max_bounces: 1,
    clamp_indirect: 0.,
    denoise: false,
    seed: 0,
};

#[test]
fn light_units_and_masks_have_their_documented_meaning() -> Result<()> {
    if !available() {
        eprintln!("skipped: no Cycles bundle (run spatial/cycles/tools/provision.sh)");
        return Ok(());
    }
    // Irradiance 3 on albedo 0.5: radiance 0.5 * 3 / pi, plus a little from the specular layer.
    let mut s = Session::new("CPU", 0)?;
    let mesh = floor(
        Material {
            diffuse: [0.5; 3],
            roughness: 1.,
            ..Default::default()
        },
        vec![],
    );
    let materials = s.materials(&mesh, &|_| Artwork::None)?;
    s.model(&mesh, &materials, 1)?;
    overhead(&mut s);
    s.distant_light([0., -1., 0.], [3.; 3], 0.);
    let f = s.render([64, 64], &RECIPE, &[1])?;
    assert!(
        (red(&f, 32) - 0.4775).abs() < 0.01,
        "distant light: {}",
        red(&f, 32)
    );
    assert_eq!(
        f.masks[0][32 * 64 + 32],
        1.,
        "the floor covers the centre pixel"
    );
    assert_eq!(f.color[(32 * 64 + 32) * 4 + 3], 1.);

    // A rect light of radiance 2, facing the camera, is seen as exactly 2.
    let mut s = Session::new("CPU", 0)?;
    overhead(&mut s);
    s.rect_light([0.; 3], [0., 0., 1.], [1., 0., 0.], [2.; 3], true);
    let f = s.render([64, 64], &RECIPE, &[])?;
    assert!(
        (red(&f, 32) - 2.).abs() < 1e-3,
        "rect light seen directly: {}",
        red(&f, 32)
    );
    Ok(())
}

#[test]
fn texture_maps_are_honoured_and_survive_a_material_edit() -> Result<()> {
    if !available() {
        return Ok(());
    }
    // Constants alone would make a magenta mirror. The maps make it rough, non-metallic and grey.
    let map = |image, channel, srgb| {
        Some(TextureMap {
            image,
            channel,
            srgb,
            scale: [1.; 4],
            bias: [0.; 4],
        })
    };
    let material = Material {
        diffuse: [1., 0., 1.],
        roughness: 0.1,
        metallic: 1.,
        maps: MaterialMaps {
            diffuse: map(0, TextureChannel::Rgb, true),
            roughness: map(1, TextureChannel::G, false),
            metallic: map(1, TextureChannel::B, false),
            normal: map(2, TextureChannel::Rgb, false),
            occlusion: None,
        },
        slot: "/m".into(),
        ..Default::default()
    };
    let images = vec![
        Image {
            source: "base".into(),
            width: 2,
            height: 1,
            rgba: vec![188, 188, 188, 255, 99, 99, 99, 255],
        },
        Image {
            source: "data".into(),
            width: 1,
            height: 1,
            rgba: vec![0, 255, 0, 255],
        },
        Image {
            source: "normal".into(),
            width: 1,
            height: 1,
            rgba: vec![128, 128, 255, 255],
        },
    ];
    let mesh = floor(material.clone(), images);
    let mut s = Session::new("CPU", 0)?;
    let materials = s.materials(&mesh, &|_| Artwork::None)?;
    s.model(&mesh, &materials, 1)?;
    overhead(&mut s);
    s.distant_light([0., -1., 0.], [3.; 3], 0.);
    // Columns 21 and 43 look at the two texel centres: linear 0.50 and 0.125.
    let f = s.render([64, 64], &RECIPE, &[])?;
    assert!(
        (red(&f, 21) - 0.4775).abs() < 0.02,
        "bright texel {}",
        red(&f, 21)
    );
    assert!(
        (red(&f, 43) - 0.1194).abs() < 0.03,
        "dark texel {}",
        red(&f, 43)
    );
    let green = f.color[(32 * 64 + 21) * 4 + 1];
    assert!(
        (green - red(&f, 21)).abs() < 1e-3,
        "neutral, not the magenta constant"
    );
    s.update_material(
        materials[0],
        &Material {
            roughness: 0.9,
            ..material
        },
        Artwork::None,
    );
    let g = s.render([64, 64], &RECIPE, &[])?;
    assert!(
        (red(&g, 21) - red(&f, 21)).abs() < 0.01,
        "maps kept after an edit"
    );
    Ok(())
}
