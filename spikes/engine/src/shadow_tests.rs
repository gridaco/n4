use crate::{
    graphics::{Graphic, PaintCache},
    render::gpu::{FrameInputs, Gpu},
};
use anyhow::Result;
use serde_json::{Value, json};

const WIDTH: usize = 24;
const HEIGHT: usize = 18;
const GAUSSIAN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/creator/gaussian-blur.wgsl"
);

fn render(gpu: &Gpu, layers: Value, frame: u32) -> Result<Vec<u8>> {
    let graphics: Vec<Graphic> = serde_json::from_value(layers)?;
    let base = [0, 0, 0, 255].repeat(WIDTH * HEIGHT);
    let pixels = gpu
        .compose_exposed(
            FrameInputs {
                size: [WIDTH as u32, HEIGHT as u32],
                primary: crate::render::gpu::PrimaryPicture::Video(&base),
                video_layers: &Default::default(),
                overlay: None,
                scopes: vec![(&graphics, frame)],
            },
            None,
            &mut PaintCache::default(),
        )?
        .0;
    Ok(pixels)
}

fn path(d: &str, rgba: [u8; 4]) -> Value {
    json!({"content":{"type":"path", "d":d, "fill":rgba}})
}

fn underlay(offset: [i32; 2], sigma: f64, rgba: [u8; 4]) -> Value {
    json!({"shader":GAUSSIAN, "sigma_px":sigma, "offset":offset, "rgba":rgba})
}

#[test]
fn scratch_budget_uses_the_visible_caster_and_requested_kernel() -> Result<()> {
    let mut project: crate::Project = serde_json::from_str(include_str!("../fixtures/title.json"))?;
    project.size = [4096, 4096];
    let mut caster = json!({"content":{"type":"footage"},
                            "underlay":underlay([0,0], 12., [0,0,0,255])});
    let mut admit = |layer: &Value| -> Result<()> {
        project.graphics = serde_json::from_value(json!([layer]))?;
        Gpu::validate_shadow_scratch(&project, &PaintCache::default())
    };
    assert!(admit(&caster).unwrap_err().to_string().contains("256 MiB"));
    caster["underlay"]["sigma_px"] = json!(0);
    admit(&caster)?;
    caster["underlay"]["sigma_px"] = json!(12);
    caster["mask"] = json!({"content":{"type":"path", "d":"M8 8H72V72H8Z"}});
    admit(&caster)?;
    Ok(())
}

fn linear(byte: u8) -> f64 {
    let signal = f64::from(byte) / 255.;
    if signal <= 0.04045 {
        signal / 12.92
    } else {
        ((signal + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(value: f64) -> u8 {
    let signal = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    };
    (255. * signal).round() as u8
}

// Compute the reference in unbounded caster space before cropping to the stage.
fn gaussian(x: i32, y: i32, sigma: f64, alpha: impl Fn(i32, i32) -> f64) -> f64 {
    if sigma == 0. {
        return alpha(x, y);
    }
    let radius = (3. * sigma).ceil() as i32;
    let mut sum = 0.;
    let mut total = 0.;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let weight = (-f64::from(dx * dx + dy * dy) / (2. * sigma * sigma)).exp();
            sum += weight * alpha(x + dx, y + dy);
            total += weight;
        }
    }
    sum / total
}

#[test]
fn masked_translucent_caster_matches_shadow_convolution_beyond_source_and_stage() -> Result<()> {
    let gpu = Gpu::new()?;
    let tint = [205, 102, 53, 192];
    for (origin, delta, sigma) in [([-3, 3], [6, 3], 1.5), ([9, 8], [-4, -3], 0.)] {
        let mut caster = path("M0 0H9V6H0Z", [255, 255, 255, 128]);
        caster["position"] = json!(origin);
        caster["mask"] = json!({"content":{"type":"path", "d":"M0.5 0H7V6H0.5Z"}});
        caster["underlay"] = underlay(delta, sigma, tint);
        let pixels = render(&gpu, json!([caster]), 0)?;
        let coverage = |x: i32, y: i32| {
            let (x, y) = (x - origin[0], y - origin[1]);
            if !(0..7).contains(&x) || !(0..6).contains(&y) {
                return 0.;
            }
            (128. / 255.) * if x == 0 { 0.5 } else { 1. }
        };
        for y in 0..HEIGHT as i32 {
            for x in 0..WIDTH as i32 {
                let index = 4 * (y as usize * WIDTH + x as usize);
                let alpha = coverage(x, y);
                let shade = gaussian(x - delta[0], y - delta[1], sigma, coverage)
                    * f64::from(tint[3])
                    / 255.;
                for channel in 0..3 {
                    let expected = encode(alpha + (1. - alpha) * shade * linear(tint[channel]));
                    assert!(
                        pixels[index + channel].abs_diff(expected) <= 2,
                        "at ({x},{y}), channel {channel}, sigma {sigma}: {} vs {expected}",
                        pixels[index + channel]
                    );
                }
                assert_eq!(pixels[index + 3], 255);
            }
        }
    }
    Ok(())
}

#[test]
fn group_casts_one_finished_silhouette_and_later_art_covers_the_shadow() -> Result<()> {
    let gpu = Gpu::new()?;
    let mut first = path("M1 1H7V4H1Z", [255, 255, 255, 128]);
    first["wipe"] = json!([0, 5]);
    let second = path("M4 1H10V4H4Z", [255, 255, 255, 128]);
    let mut group = json!({"position":[2,1], "content":{"type":"group","children":[first,second]}});
    let dir = tempfile::tempdir()?;
    let fade = dir.path().join("fade.wgsl");
    std::fs::write(
        &fade,
        "fn effect(p: vec2<i32>, c: vec4<f32>, a: f32) -> vec4<f32> { return c * a; }",
    )?;
    group["filter"] = json!({"shader": fade, "amount":50});
    group["underlay"] = underlay([0, 6], 0., [255, 255, 255, 255]);
    let foreground = path("M11 7H14V12H11Z", [255, 0, 0, 255]);
    let pixels = |frame: u32| render(&gpu, json!([group, foreground]), frame);
    let before = pixels(0)?;
    let after = pixels(5)?;
    let sample = |picture: &[u8], x: usize| -> [u8; 4] {
        let i = 4 * (9 * WIDTH + x);
        picture[i..i + 4].try_into().unwrap()
    };
    let half = 128. / 255.;
    let both = encode(0.5 * (half + (1. - half) * half));
    assert_eq!(sample(&before, 3), [0, 0, 0, 255]);
    assert!(sample(&before, 7)[0].abs_diff(encode(0.5 * half)) <= 2);
    assert!(sample(&after, 3)[0].abs_diff(encode(0.5 * half)) <= 2);
    assert!(sample(&after, 7)[0].abs_diff(both) <= 2);
    assert_eq!(sample(&after, 11), [255, 0, 0, 255]);
    Ok(())
}
