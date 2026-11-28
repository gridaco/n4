use crate::{
    graphics::{Exposure, Graphic, PaintCache},
    render::gpu::{FrameInputs, Gpu},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn adjustment(shader: &Path, sigma_px: f32, contour: &str) -> Value {
    json!({
        "content": {"type": "backdrop", "filter": {"shader": shader, "sigma_px": sigma_px}},
        "mask": {"content": {"type": "path", "d": contour}}
    })
}

fn gaussian() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/creator/gaussian-blur.wgsl"
    ))
}

fn picture(
    gpu: &Gpu,
    size: [u32; 2],
    video: &[u8],
    layers: Value,
    exposure: Option<Exposure>,
) -> Result<Vec<u8>> {
    let graphics: Vec<Graphic> = serde_json::from_value(layers)?;
    let inputs = FrameInputs {
        size,
        primary: crate::render::gpu::PrimaryPicture::Video(video),
        video_layers: &Default::default(),
        scopes: vec![(&graphics, 0)],
        overlay: None,
    };
    Ok(gpu
        .compose_exposed(inputs, exposure, &mut PaintCache::default())?
        .0)
}

fn linear(byte: u8) -> f64 {
    let signal = f64::from(byte) / 255.;
    if signal <= 0.04045 {
        signal / 12.92
    } else {
        ((signal + 0.055) / 1.055).powf(2.4)
    }
}

fn encoded(value: f64) -> u8 {
    let signal = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    };
    (255. * signal).round() as u8
}

// Independent 2D convolution over the full plate. The GPU implementation
// uses two separable passes scoped to the mask and its sampling margin.
fn convolve(plate: &[u8], size: [u32; 2], x: u32, y: u32, sigma: f64) -> f64 {
    let radius = (sigma * 3.).ceil() as i32;
    let mut weighted = 0.;
    let mut sum = 0.;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let weight = (-f64::from(dx * dx + dy * dy) / (2. * sigma * sigma)).exp();
            let sx = (x as i32 + dx).clamp(0, size[0] as i32 - 1) as u32;
            let sy = (y as i32 + dy).clamp(0, size[1] as i32 - 1) as u32;
            weighted += weight * linear(plate[((sy * size[0] + sx) * 4) as usize]);
            sum += weight;
        }
    }
    weighted / sum
}

#[test]
fn cropped_gaussian_matches_full_plate_convolution_through_fractional_mask() -> Result<()> {
    let gpu = Gpu::new()?;
    let size = [23, 17];
    let mut plate = [0, 0, 0, 255].repeat(23 * 17);
    for (x, y) in [(0, 5), (13, 10), (6, 13), (3, 16)] {
        let i = 4 * (y * 23 + x);
        plate[i..i + 3].fill(255);
    }
    let card = "M-4 5.5 H12.5 V19 H-4 Z";
    let draw = |sigma| {
        picture(
            &gpu,
            size,
            &plate,
            json!([adjustment(gaussian(), sigma, card)]),
            None,
        )
    };
    assert_eq!(draw(0.)?, plate);
    let blurred = draw(1.25)?;
    let dir = tempfile::tempdir()?;
    let svg = dir.path().join("window.svg");
    fs::write(
        &svg,
        format!(r##"<svg width="23" height="17"><path d="{card}" fill="#ffffff"/></svg>"##),
    )?;
    let mut svg_card = adjustment(gaussian(), 1.25, card);
    svg_card["mask"]["content"] = json!({"type":"svg","source":svg});
    // An SVG is painted on the CPU and a path on the device: the same picture on one painter,
    // and within a rounding step of each other on the default pair.
    let cpu = Gpu::reference()?;
    assert_eq!(
        picture(&cpu, size, &plate, json!([svg_card.clone()]), None)?,
        picture(
            &cpu,
            size,
            &plate,
            json!([adjustment(gaussian(), 1.25, card)]),
            None
        )?
    );
    assert_eq!(
        crate::render::skia_gpu::bytes_beyond_edge_rounding(
            &picture(&gpu, size, &plate, json!([svg_card]), None)?,
            &blurred,
        ),
        0
    );
    for y in 0..size[1] {
        for x in 0..size[0] {
            let i = ((y * size[0] + x) * 4) as usize;
            let mx = (12.5 - f64::from(x)).clamp(0., 1.);
            let my = (f64::from(y + 1) - 5.5).clamp(0., 1.);
            let coverage = mx * my;
            if coverage == 0. {
                assert_eq!(&blurred[i..i + 4], &plate[i..i + 4], "outside ({x},{y})");
            } else {
                let expected = encoded(
                    linear(plate[i]) * (1. - coverage)
                        + convolve(&plate, size, x, y, 1.25) * coverage,
                );
                for value in &blurred[i..i + 3] {
                    assert!(
                        value.abs_diff(expected) <= 2,
                        "({x},{y}) got {value} expected {expected}"
                    );
                }
                assert_eq!(blurred[i + 3], 255);
            }
        }
    }
    // The impulse at x=13 lives outside the mask. Its light reaches x=12.
    assert!(blurred[(10 * 23 + 12) * 4] > 20);
    Ok(())
}

#[test]
fn adjustments_observe_order_group_isolation_and_premultiplied_replacement() -> Result<()> {
    let gpu = Gpu::new()?;
    let size = [20, 20];
    let mut plate = vec![255; 20 * 20 * 4];
    for y in 0..20 {
        for x in 0..20 {
            let i = (y * 20 + x) * 4;
            plate[i] = (12 * x) as u8;
            plate[i + 1] = (10 * y) as u8;
        }
    }
    let bounds = "M0 0H20V20H0Z";
    let dir = tempfile::tempdir()?;
    let shader = dir.path().join("neighbor.wgsl");
    fs::write(
        &shader,
        "fn filter_pixel(p: vec2<i32>) -> vec4<f32> { return tap(p, 1); }",
    )?;
    let shift = adjustment(&shader, 1., bounds);
    let ordered = picture(&gpu, size, &plate, json!([shift, shift]), None)?;
    let pixel = |x: usize, y: usize| (y * 20 + x) * 4;
    let i = pixel(5, 5);
    assert_eq!(&ordered[i..i + 4], &plate[pixel(7, 7)..pixel(7, 7) + 4]);
    let group = |children: Value| json!([{"content":{"type":"group","children":children}}]);
    let wash = json!({"content":{"type":"path","d":bounds,"fill":[180,30,60,128]}});
    let unfiltered = picture(&gpu, size, &plate, group(json!([wash])), None)?;
    let isolated = picture(&gpu, size, &plate, group(json!([wash, shift])), None)?;
    // The inner pixels are a uniform translucent red before compositing on the
    // colored plate. A second source-over or sampling the plate changes them.
    assert_eq!(&isolated[i..i + 4], &unfiltered[i..i + 4]);
    Ok(())
}

#[test]
fn animated_mask_samples_exposure_time_and_preserves_its_path_edges() -> Result<()> {
    let gpu = Gpu::new()?;
    let size = [16, 16];
    let mut plate = [0, 0, 0, 255].repeat(16 * 16);
    let i = (7 * 16 + 7) * 4;
    plate[i..i + 3].fill(255);
    let mut effect = adjustment(gaussian(), 1.5, "M2.5 0H12V16H2.5Z");
    let always_on = picture(&gpu, size, &plate, json!([effect]), None)?;
    let dir = tempfile::tempdir()?;
    let shader = dir.path().join("pulse.wgsl");
    fs::write(
        &shader,
        "fn coverage_amount(local: vec2<f32>, phase: f32, alpha: f32) -> f32 { return select(0., alpha, phase >= 0.5); }",
    )?;
    effect["mask"]["shader"] = json!({"shader":shader,"period_frames":1});
    let doc = json!([effect]);
    assert_eq!(picture(&gpu, size, &plate, doc.clone(), None)?, plate);
    let exposed = picture(
        &gpu,
        size,
        &plate,
        doc,
        Some(Exposure {
            samples: 2,
            angle_degrees: 360,
        }),
    )?;
    for (pixel, (a, b)) in exposed.iter().zip(plate.iter().zip(always_on.iter())) {
        let mean = encoded((linear(*a) + linear(*b)) / 2.);
        assert!(
            pixel.abs_diff(mean) <= 1,
            "exposure got {pixel} expected {mean}"
        );
    }
    assert!(exposed[i] < 220 && exposed[i] > 180);
    Ok(())
}
