use crate::{
    camera::Camera,
    graphics::{Graphic, PaintCache},
    render::gpu::{FrameInputs, Gpu, PrimaryPicture},
};
use anyhow::Result;
use nothing_spatial::{Material, Mesh, Vertex};
use serde_json::json;
use std::{collections::HashMap, path::Path, sync::Arc};

#[test]
fn tilted_mesh_passes_through_editable_type_and_stays_resident_when_seeking() -> Result<()> {
    let gpu = Gpu::new()?;
    let material = Material {
        diffuse: [0.; 3],
        emissive: [1., 0., 0.],
        specular: [0.; 3],
        roughness: 1.,
        metallic: 0.,
        clearcoat: 0.,
        clearcoat_roughness: 1.,
        use_specular_workflow: false,
        ..Default::default()
    };
    // One red surface spans both sides of the ink plane. The right edge is closer to the camera.
    let vertices = [
        [-2., -2., -0.8],
        [2., -2., 0.8],
        [2., 2., 0.8],
        [-2., 2., -0.8],
    ]
    .map(|position| Vertex {
        position,
        normal: [0., 0., 1.],
        material: 0,
        uv: [0.; 2],
    })
    .to_vec();
    let mesh = Mesh {
        vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
        materials: vec![material],
        ..Default::default()
    };
    let model = json!({"id":"sheet","source":"depth-fixture.usda","prim":"/Card","rotation_y":[{"frame":0,"value":0},{"frame":10,"value":160}]});
    let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fonts/N4GridSans.ttf");
    let text = json!({"id":"glyphs","position":[-0.96,0.64,0],"text":{"content":{"type":"label","font":font,
        "text":"OO","size":42,"width":110,"rgb":[255,255,255]}}});
    let scene = |models| -> Result<Graphic> {
        Ok(serde_json::from_value(json!({"content":{"type":"scene3d",
        "pixels_per_meter":36,"models":models,"planes":[text]}}))?)
    };
    let combined = scene(json!([model]))?;
    let text_only = scene(json!([]))?;
    let camera: Camera = serde_json::from_value(
        json!({"id":"view","projection":"perspective","position":[0,0,3],"fov_y":42}),
    )?;
    let mut cache = PaintCache::default();
    cache.meshes.insert(
        ("depth-fixture.usda".into(), "/Card".into()),
        Arc::new(mesh),
    );
    let size = [96, 80];
    let black = [0, 0, 0, 255].repeat((size[0] * size[1]) as usize);
    let mut draw = |layer: &Graphic, frame, view: &Camera| {
        gpu.compose_view(
            FrameInputs {
                size,
                primary: PrimaryPicture::Video(&black),
                video_layers: &HashMap::new(),
                overlay: None,
                scopes: vec![(std::slice::from_ref(layer), frame)],
            },
            None,
            Some(view),
            &mut cache,
        )
    };
    let (reference, _) = draw(&text_only, 0, &camera)?;
    let (picture, cold) = draw(&combined, 0, &camera)?;
    let mut ink_in_front = 0;
    let mut mesh_in_front = 0;
    for (i, (ink, mixed)) in reference
        .chunks_exact(4)
        .zip(picture.chunks_exact(4))
        .enumerate()
    {
        if ink[1] < 250 {
            continue;
        }
        let x = i as u32 % size[0];
        if x < size[0] / 2 - 2 && mixed[1] > 250 {
            ink_in_front += 1;
        }
        if x > size[0] / 2 + 2 && mixed == [255, 0, 0, 255] {
            mesh_in_front += 1;
        }
    }
    assert!(
        ink_in_front > 30 && mesh_in_front > 30,
        "front ink pixels {ink_in_front}, occluded ink pixels {mesh_in_front}"
    );
    assert_eq!((cold.spatial_passes, cold.mesh_uploads), (1, 1));
    let (turned, warm) = draw(&combined, 10, &camera)?;
    assert_ne!(picture, turned);
    assert_eq!((warm.mesh_uploads, warm.graphic_uploads), (0, 0));
    let shifted: Camera = serde_json::from_value(
        json!({"id":"view","projection":"perspective","position":[0.25,0,3],"fov_y":42}),
    )?;
    let (reframed, edit) = draw(&combined, 0, &shifted)?;
    assert_ne!(picture, reframed);
    assert_eq!(
        (edit.mesh_uploads, edit.graphic_uploads, edit.raster_paints),
        (0, 0, 0)
    );
    assert_eq!(picture, draw(&combined, 0, &camera)?.0);

    // A different WGSL client displays coverage directly in RGB. Beauty and
    // isolated ink supply independent references for the winning samples.
    let tmp = tempfile::tempdir()?;
    let shader = tmp.path().join("view-mattes.wgsl");
    std::fs::write(
        &shader,
        "fn effect(p:vec2<i32>, c:vec4<f32>, a:f32)->vec4<f32> {
        return vec4(matte_word(p),matte_word_behind_sheet(p),matte_sheet_behind_word(p),1.);
    }",
    )?;
    let mattes = json!([
        {"name":"word","object":"glyphs"},
        {"name":"word_behind_sheet","object":"glyphs","occluded_by":"sheet"},
        {"name":"sheet_behind_word","object":"sheet","occluded_by":"glyphs"}
    ]);
    let mut observed = combined.clone();
    observed.filter = Some(serde_json::from_value(
        json!({"shader":shader,"amount":100,"mattes":mattes}),
    )?);
    let (visible, counts) = draw(&observed, 0, &camera)?;
    assert_eq!(counts.object_matte_passes, 3);
    assert_eq!(
        counts.object_matte_scratch_bytes,
        u64::from(size[0]) * u64::from(size[1]) * 11
    );
    assert_eq!(
        (cold.object_matte_passes, cold.object_matte_scratch_bytes),
        (0, 0)
    );
    let mut partial = 0;
    let mut front = 0;
    let mut back = 0;
    for ((ink, beauty), mask) in reference
        .chunks_exact(4)
        .zip(picture.chunks_exact(4))
        .zip(visible.chunks_exact(4))
    {
        assert!(
            ink[1].abs_diff(mask[0]) <= 2,
            "projected glyph coverage changed: {ink:?} {mask:?}"
        );
        if (8..245).contains(&mask[0]) {
            partial += 1;
        }
        if ink[1] == 255 && beauty[..3] == [255, 0, 0] {
            assert_eq!(&mask[..3], &[255, 255, 0]);
            back += 1;
        }
        if ink[1] == 255 && beauty[..3] == [255, 255, 255] && mask[2] == 255 {
            front += 1;
        }
    }
    assert!(partial > 10 && front > 30 && back > 30);

    // Draw a third unaddressed card over both objects. Projected ink survives
    // and neither named object owns any of the covered samples.
    let mut foreground = model.clone();
    foreground.as_object_mut().unwrap().remove("id");
    foreground["position"] = json!([0, 0, 1.4]);
    let mut covered = scene(json!([model, foreground]))?;
    covered.filter = observed.filter.clone();
    let (masked, _) = draw(&covered, 0, &camera)?;
    for (a, b) in visible.chunks_exact(4).zip(masked.chunks_exact(4)) {
        assert_eq!(a[0], b[0], "projected ink depends on other geometry");
        if a[0] == 255 {
            assert_eq!(&b[1..3], &[0, 0], "third card leaked {b:?}");
        }
    }

    // With two objects, every projected glyph sample is either visible or
    // owned by the other object. A third winner can remove samples from both.
    let ownership_shader = tmp.path().join("visibility.wgsl");
    std::fs::write(
        &ownership_shader,
        "fn effect(p:vec2<i32>, c:vec4<f32>, a:f32)->vec4<f32> {
        return vec4(matte_word(p),matte_visible(p),matte_hidden(p),1.);
    }",
    )?;
    let ownership_filter = Some(serde_json::from_value(json!({
        "shader":ownership_shader, "amount":100, "mattes":[
            {"name":"word","object":"glyphs"},
            {"name":"visible","object":"glyphs","coverage":"visible"},
            {"name":"hidden","object":"glyphs","occluded_by":"sheet"}
        ]
    }))?);
    let mut ownership = combined.clone();
    ownership.filter = ownership_filter.clone();
    let (classified, _) = draw(&ownership, 0, &camera)?;
    for (classification, beauty) in classified.chunks_exact(4).zip(picture.chunks_exact(4)) {
        assert!(classification[1].abs_diff(beauty[1]) <= 2);
        let linear = |c: u8| super::linear(c);
        let sum = linear(classification[1]) + linear(classification[2]);
        assert!((linear(classification[0]) - sum).abs() < 0.015);
    }
    covered.filter = ownership_filter;
    let (classified, _) = draw(&covered, 0, &camera)?;
    assert!(classified.chunks_exact(4).all(|p| p[1] == 0 && p[2] == 0));
    // An equal-depth object drawn earlier wins the same strict depth test as
    // the beauty pass. Equal-depth duplicates drawn later never own a sample.
    let mut twin = model.clone();
    twin.as_object_mut().unwrap().remove("id");
    let mut twin_first = scene(json!([twin, model]))?;
    twin_first.filter = observed.filter.clone();
    let (tied, _) = draw(&twin_first, 0, &camera)?;
    assert!(tied.chunks_exact(4).all(|p| p[1] == 0));
    let mut twin_last = scene(json!([model, twin]))?;
    twin_last.filter = observed.filter.clone();
    assert_eq!(visible, draw(&twin_last, 0, &camera)?.0);

    // A projected-only binding uses no owner attachment, and a time-gated
    // plane leaves an empty output instead of reusing its previous mask.
    let plain_shader = tmp.path().join("projection.wgsl");
    std::fs::write(
        &plain_shader,
        "fn effect(p:vec2<i32>, c:vec4<f32>, a:f32)->vec4<f32> {
        return vec4(vec3(matte_word(p)),1.); }",
    )?;
    let mut limited_text = text.clone();
    limited_text["text"]["span"] = json!([0, 1]);
    let limited: Graphic = serde_json::from_value(json!({
        "content": {"type":"scene3d","pixels_per_meter":36,"models":[model],"planes":[limited_text]},
        "filter":{"shader":plain_shader,"amount":100,"mattes":[{"name":"word","object":"glyphs"}]}
    }))?;
    let (projected, counts) = draw(&limited, 0, &camera)?;
    for (a, b) in projected.chunks_exact(4).zip(visible.chunks_exact(4)) {
        assert_eq!(a[0], b[0]);
    }
    assert_eq!(counts.object_matte_passes, 1);
    assert_eq!(
        counts.object_matte_scratch_bytes,
        u64::from(size[0]) * u64::from(size[1]) * 5
    );
    let (inactive, _) = draw(&limited, 1, &camera)?;
    assert!(inactive.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]));
    Ok(())
}

#[test]
fn an_image_material_keys_the_picture_accepted_before_the_source_changed() -> Result<()> {
    use crate::{Project, graphics::PreparedRasterAssets};

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    for (original, replacement, kind) in [
        ("webp/decal.webp", "webp/linear-rgb.webp", "image"),
        ("webp/patches.webp", "webp/animated.webp", "video"),
        ("gif/restore.gif", "webp/patches.webp", "video"),
    ] {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("screen.bin");
        std::fs::copy(fixtures.join(original), &source)?;
        let content = if kind == "image" {
            json!({"type":"image","source":source})
        } else {
            json!({"type":"video","source":source,"start":0,"source_in":0,"duration":1,"size":[4,2]})
        };
        let layer = json!({"content":content});
        let project: Project = serde_json::from_value(json!({
            "size":[32,24], "fps":[24,1],
            "shots":[{"duration":1,"background":{"color":[0,0,0]},"graphics":[layer]}]
        }))?;
        let key = |cache: &mut PaintCache| -> Result<serde_json::Value> {
            let mut value = layer.clone();
            super::identify_files(
                &mut value,
                &mut cache.file_digests,
                &cache.pinned_images,
                &cache.animations,
                &mut false,
            )?;
            Ok(value)
        };
        let mut accepted = PaintCache::default();
        accepted.install_raster_assets(&PreparedRasterAssets::prepare_pinned(&project)?);
        // Even the first Final-key request can happen after the original file is gone.
        std::fs::remove_file(&source)?;
        let before = key(&mut accepted)?;
        let expected_prefix = if kind == "image" {
            "raster:"
        } else {
            "animation:"
        };
        assert!(
            before["content"]["source"]
                .as_str()
                .unwrap()
                .starts_with(expected_prefix)
        );
        std::fs::copy(fixtures.join(replacement), &source)?;
        accepted.clear_preview_caches();
        assert_eq!(key(&mut accepted)?, before);
        let mut next = PaintCache::default();
        next.install_raster_assets(&PreparedRasterAssets::prepare_pinned(&project)?);
        assert_ne!(key(&mut next)?, before);
    }
    Ok(())
}
