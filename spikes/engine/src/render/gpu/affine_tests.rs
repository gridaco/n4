use super::*;
use serde_json::json;

#[test]
fn enlarged_caster_is_admitted_against_transformed_scratch_bounds() -> Result<()> {
    let mut project: crate::Project =
        serde_json::from_str(include_str!("../../../fixtures/title.json"))?;
    project.size = [4096, 4096];
    let mut layer: Graphic = serde_json::from_value(json!({
        "content":{"type":"path","d":"M0 0H100V100H0Z","fill":[0,255,0,255]},
        "underlay":{"shader":"gaussian.wgsl","sigma_px":8,"offset":[8,-3],"rgba":[0,0,0,255]}
    }))?;
    project.graphics = vec![layer.clone()];
    Gpu::validate_shadow_scratch(&project, &PaintCache::default())?;
    layer.transform = Some(Transform {
        scale: Some(41.),
        ..Default::default()
    });
    project.graphics = vec![layer];
    assert!(
        Gpu::validate_shadow_scratch(&project, &PaintCache::default())
            .unwrap_err()
            .to_string()
            .contains("256 MiB")
    );
    Ok(())
}

#[test]
fn camera_zoom_counts_toward_shadow_and_backdrop_admission_only_when_selected() -> Result<()> {
    let mut project: crate::Project =
        serde_json::from_str(include_str!("../../../fixtures/title.json"))?;
    project.size = [4096, 4096];
    let shot = &mut project.shots[0];
    shot.cameras.push(serde_json::from_value(json!({
        "id":"close","projection":"orthographic","center_x":50,"center_y":50,"zoom":42
    }))?);
    shot.graphics = serde_json::from_value(json!([
       {"content":{"type":"path","d":"M0 0H100V100H0Z","fill":[0,0,0,255]},
        "underlay":{"shader":"gaussian.wgsl","sigma_px":8,"offset":[2,2],"rgba":[0,0,0,255]}},
       {"content":{"type":"backdrop","filter":{"sigma_px":2,"shader":"gaussian.wgsl"}},
        "mask":{"content":{"type":"path","d":"M0 0H100V100H0Z"}}}
    ]))?;
    let cache = PaintCache::default();
    Gpu::validate_backdrop_scratch(&project, &cache)?;
    Gpu::validate_shadow_scratch(&project, &cache)?;
    project.shots[0].camera = Some("close".into());
    for result in [
        Gpu::validate_backdrop_scratch(&project, &cache),
        Gpu::validate_shadow_scratch(&project, &cache),
    ] {
        assert!(result.unwrap_err().to_string().contains("256 MiB"));
    }
    Ok(())
}

#[test]
fn a_quarter_turn_rotates_the_finished_wipe_mask_and_local_effect_around_the_pivot() -> Result<()> {
    let gpu = Gpu::new()?;
    let mut cache = PaintCache::default();
    let folder = tempfile::tempdir()?;
    let shader = folder.path().join("bands.wgsl");
    std::fs::write(
        &shader,
        "fn effect(p:vec2<i32>, c:vec4<f32>, a:f32) -> vec4<f32> { return vec4(c.r * f32(p.x % 3) / 2., c.gb, c.a); }",
    )?;
    let mut layer: Graphic = serde_json::from_value(json!({
        "position":[5,6], "content":{"type":"path","d":"M2 1H18V13H2Z", "fill":[240,120,80,255]},
        "mask":{"position":[1,-1],"content":{"type":"path","d":"M2.3 3H19V10.4H2.3Z"}},
        "wipe":[1,5], "filter":{"shader":shader,"amount":100}
    }))?;
    let plate = [0, 0, 0, 255].repeat(32 * 32);
    let mut render = |layer: &Graphic| {
        gpu.compose_scene(
            [32; 2],
            &plate,
            None,
            std::slice::from_ref(layer),
            3,
            &mut cache,
        )
    };
    let (before, _) = render(&layer)?;
    assert_ne!(before, plate);
    layer.transform = Some(Transform {
        pivot: [11., 10.],
        rotation_degrees: Some(90.),
        ..Default::default()
    });
    let (after, _) = render(&layer)?;
    for y in 0..32 {
        for x in 0..32 {
            let src = 4 * (y * 32 + x);
            let dst = 4 * (x * 32 + 31 - y);
            assert_eq!(
                before[src..src + 4],
                after[dst..dst + 4],
                "source ({x}, {y})"
            );
        }
    }
    Ok(())
}

#[test]
fn scaling_a_transformed_caster_keeps_shadow_offset_in_output_pixels() -> Result<()> {
    let gpu = Gpu::new()?;
    let mut cache = PaintCache::default();
    let gaussian =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/gaussian-blur.wgsl");
    let layer: Graphic = serde_json::from_value(json!({
        "position":[8,9], "content":{"type":"path","d":"M0 0H4V3H0Z","fill":[0,255,0,255]},
        "transform":{"pivot":[2,1],"scale":2},
        "underlay":{"shader":gaussian,"sigma_px":0,"offset":[8,-3],"rgba":[255,0,0,255]}
    }))?;
    let plate = [0, 0, 0, 255].repeat(32 * 32);
    let (actual, _) = gpu.compose_scene([32; 2], &plate, None, &[layer], 0, &mut cache)?;
    let sample = |x: usize, y: usize| &actual[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
    assert_eq!(sample(7, 9), &[0, 255, 0, 255]);
    assert_eq!(sample(15, 6), &[255, 0, 0, 255]);
    assert_eq!(sample(23, 6), &[0, 0, 0, 255]);
    Ok(())
}

#[test]
fn a_stretched_parent_carries_the_mask_on_isolated_art_and_on_the_backdrop() -> Result<()> {
    let gaussian =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/creator/gaussian-blur.wgsl");
    let mask = json!({"position":[1,-2],"content":{"type":"path","d":"M-6 -1H7V4H-6Z"}});
    let isolated = json!({"mask":mask,"content":{"type":"group","children":[
        {"content":{"type":"path","d":"M-16 -16H16V16H-16Z","fill":[255,0,0,255]}}
    ]}});
    let backdrop = json!({"content":{"type":"transform_group","children":[
        {"mask":mask,"content":{"type":"backdrop","filter":{"sigma_px":2,"shader":gaussian}}}
    ]}});
    let checker: Vec<_> = (0..64 * 64)
        .flat_map(|i| {
            let v = if (i % 64 + i / 64) % 2 == 0 { 0 } else { 255 };
            [v, v, v, 255]
        })
        .collect();
    let gpu = Gpu::new()?;
    let mut cache = PaintCache::default();
    for (mut child, frosted) in [(isolated, false), (backdrop, true)] {
        child["position"] = json!([2, 4]);
        child["transform"] = json!({"rotation_degrees":30});
        let group: Graphic = serde_json::from_value(json!({
            "position":[32,23], "transform":{"scale_x":2},
            "content":{"type":"transform_group","children":[child]}
        }))?;
        let (image, stats) = gpu.compose_scene([64; 2], &checker, None, &[group], 0, &mut cache)?;
        assert_eq!(stats.backdrop_passes, if frosted { 3 } else { 0 });
        let mut inside = 0;
        for y in 0..64 {
            for x in 0..64 {
                // Undo parent stretch then child rotation. The child's origin
                // is (32 + 2*2, 23 + 4), and the shifted mask is [-5,8] × [-3,2].
                let dx = (x as f64 + 0.5 - 36.) / 2.;
                let dy = y as f64 + 0.5 - 27.;
                let cos = 3f64.sqrt() / 2.;
                let (u, v) = (cos * dx + 0.5 * dy, -0.5 * dx + cos * dy);
                let i = (y * 64 + x) * 4;
                let pixel = &image[i..i + 4];
                if (-4.0..7.).contains(&u) && (-2.0..1.).contains(&v) {
                    inside += 1;
                    if frosted {
                        assert!(
                            (186..=189).contains(&pixel[0]),
                            "backdrop {x},{y}: {pixel:?}"
                        );
                    } else {
                        assert_eq!(pixel, &[255, 0, 0, 255], "isolated {x},{y}");
                    }
                } else if !(-6.0..9.).contains(&u) || !(-4.0..3.).contains(&v) {
                    assert_eq!(pixel, &checker[i..i + 4], "outside {x},{y}");
                }
            }
        }
        assert!(inside > 50);
    }
    Ok(())
}

#[test]
fn admission_covers_the_rotation_arc_between_keys() -> Result<()> {
    let layer: Graphic = serde_json::from_value(json!({
        "transform":{"pivot":[0.25,1.5],"scale_x":1.6},
        "channels":{"rotation":[{"frame":0,"value":-41},{"frame":20,"value":77}],
                    "scale_y":[{"frame":0,"value":1.1},{"frame":20,"value":2.3}]},
        "content":{"type":"transform_group","children":[]}
    }))?;
    let origin = [21, 30];
    let strip = Bounds::new([35, 30], [180, 3]);
    let envelope = strip.motion_envelope(origin, &layer);
    // The strip sweeps the right half of its orbit. An endpoint-only estimate
    // misses the furthest right edge, and an entire disc includes the left half.
    assert!(envelope.left >= origin[0]);
    for i in 0..=80 {
        let pose = layer.motion_at(i / 4, f64::from(i % 4) / 4.);
        let at = strip.transformed(origin, SampledTransform::new(layer.transform, pose));
        assert!(envelope.intersect(at) == at, "quarter frame {i}");
    }
    Ok(())
}
