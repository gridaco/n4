use super::*;
use serde_json::json;

#[test]
fn group_knockout_uses_source_alpha_and_reveals_the_underlying_picture() -> Result<()> {
    let group: Graphic = serde_json::from_value(json!({"content": {
        "type": "group", "children": [
            {"content": {"type":"path", "d":"M0 4H24V20H0Z", "fill":[255,0,0,255]}},
            {"blend":"destination_out", "content": {
                "type":"path", "d":"M0 9H24V15H0Z", "fill":[255,255,0,255]
            }},
            {"blend":"destination_out", "content": {
                "type":"path", "d":"M0 6H24V8H0Z", "fill":[0,255,255,128]
            }}
        ]
    }}))?;
    let plate: Vec<_> = (0..24 * 24)
        .flat_map(|i| {
            if i % 24 < 12 {
                [0, 255, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        })
        .collect();
    let gpu = Gpu::new()?;
    let (image, _) = gpu.compose_scene(
        [24; 2],
        &plate,
        None,
        &[group],
        0,
        &mut PaintCache::default(),
    )?;
    for (x, channel) in [(4, 1), (16, 2)] {
        let pixel = |y: usize| &image[(y * 24 + x) * 4..(y * 24 + x) * 4 + 4];
        assert_eq!(pixel(18), &[255, 0, 0, 255]);
        let offset = (12 * 24 + x) * 4;
        assert_eq!(pixel(12), &plate[offset..offset + 4]);
        assert_eq!(pixel(0), pixel(12));
        let half = pixel(6);
        assert!((186..=190).contains(&half[0]) && (186..=190).contains(&half[channel]));
        assert_eq!((half[3 - channel], half[3]), (0, 255));
    }
    Ok(())
}
