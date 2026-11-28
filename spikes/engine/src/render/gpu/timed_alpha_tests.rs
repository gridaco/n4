use super::*;
use serde_json::json;

fn straight(pixels: Vec<u8>) -> DecodedFrame {
    DecodedFrame {
        rgba: Arc::new(pixels),
        alpha: FrameAlpha::Straight,
    }
}

fn assert_pixel(pixels: &[u8], i: usize, expected: [u8; 4]) {
    let pixel = &pixels[i * 4..][..4];
    assert!(
        pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
        "pixel {i}: {pixel:?}, expected {expected:?}"
    );
}

#[test]
fn primary_with_coverage_composites_once_over_black() -> Result<()> {
    let gpu = Gpu::new()?;
    let primary = straight(vec![250, 128, 7, 0, 255, 0, 0, 128, 0, 0, 255, 255]);
    let footage: Graphic = serde_json::from_value(json!({"content":{"type":"footage"}}))?;
    let mut cache = PaintCache::default();
    for layers in [&[][..], std::slice::from_ref(&footage)] {
        let (picture, _) = gpu.compose_view(
            FrameInputs {
                size: [3, 1],
                primary: PrimaryPicture::Decoded(&primary),
                video_layers: &HashMap::new(),
                overlay: None,
                scopes: vec![(layers, 0)],
            },
            None,
            None,
            &mut cache,
        )?;
        assert_pixel(&picture, 0, [0, 0, 0, 255]);
        assert_pixel(&picture, 1, [188, 0, 0, 255]);
        assert_pixel(&picture, 2, [0, 0, 255, 255]);
    }
    Ok(())
}

#[test]
fn straight_alpha_is_associated_before_transform_and_blur() -> Result<()> {
    let gpu = Gpu::new()?;
    let mut cache = PaintCache::default();
    let source = Path::new("overlay.webp");
    // Green hidden in fully transparent pixels must never contribute to the image.
    let inset = straight([255, 0, 0, 255, 0, 255, 0, 0].repeat(2));
    let videos = HashMap::from([((source, [2, 2], 0), inset)]);
    let mut definition = json!({
        "content": {"type":"video","source":source,"source_in":0,"start":0,"duration":1,"size":[2,2]},
        "transform": {"scale":2.0}
    });
    let plate = [0, 0, 0, 255, 255, 255, 255, 255].repeat(8);
    let mut render = |value| -> Result<Vec<u8>> {
        let overlay: Graphic = serde_json::from_value(value)?;
        Ok(gpu
            .compose_view(
                FrameInputs {
                    size: [4, 4],
                    primary: PrimaryPicture::Video(&plate),
                    video_layers: &videos,
                    overlay: None,
                    scopes: vec![(std::slice::from_ref(&overlay), 0)],
                },
                None,
                None,
                &mut cache,
            )?
            .0)
    };
    let pixels = render(definition.clone())?;
    // At x=1 and x=2 the bilinear taps cover 75% and 25% red respectively.
    assert_pixel(&pixels, 5, [255, 137, 137, 255]);
    assert_pixel(&pixels, 6, [137, 0, 0, 255]);

    definition["effects"] = json!([{"type":"source_blur","sigma":1}]);
    let blurred = render(definition)?;
    assert_ne!(pixels, blurred);
    for p in blurred.chunks_exact(4) {
        assert_eq!(
            p[1], p[2],
            "transparent green leaked through the blur: {p:?}"
        );
        assert_eq!(p[3], 255);
    }
    Ok(())
}
