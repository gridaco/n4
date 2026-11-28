use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/webp")
        .join(name)
}

#[test]
fn restores_patch_coverage_across_cold_reverse_and_reset_reads() -> Result<()> {
    let file = fixture("patches.webp");
    let anim = Animation::parse(fs::read(file)?.into_boxed_slice())?;
    assert_eq!(anim.size(), [4, 2]);
    assert_eq!(anim.durations(), [40, 80, 20]);
    assert_eq!(anim.loop_count, 0);
    // Original fixture has no declared alpha; transparent pixels come from the canvas.
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let white = [255; 4];
    let clear = [0; 4];
    let rows = [
        [red, red, clear, clear],
        [clear, clear, green, green],
        [white, white, green, green],
    ];
    let mut cursor = anim.cursor()?;
    for ordinal in [2, 0, 1, 0, 2, 1] {
        let got = cursor.canvas(ordinal)?;
        let row = rows[ordinal as usize].as_flattened();
        assert_eq!(got.as_ref(), row.repeat(2));
    }
    Ok(())
}

#[test]
fn zero_delay_frame_updates_canvas_and_one_pass_ignores_loop_count() -> Result<()> {
    let mut bytes = fs::read(fixture("patches.webp"))?;
    let anim = bytes.windows(4).position(|b| b == b"ANIM").unwrap() + 8;
    bytes[anim + 4..anim + 6].copy_from_slice(&3u16.to_le_bytes());
    let frames = bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, k)| (k == b"ANMF").then_some(i + 8))
        .collect::<Vec<_>>();
    bytes[frames[0] + 12..frames[0] + 15].fill(0);
    bytes[frames[0] + 15] &= !1; // The zero-duration red patch stays underneath frame one.
    let asset = Animation::parse(bytes.clone().into_boxed_slice())?;
    assert_eq!(asset.loop_count, 3);
    assert_eq!(asset.durations(), [0, 80, 20]);
    assert_eq!(&asset.cursor()?.canvas(1)?[..4], &[255, 0, 0, 255]);
    for f in frames {
        bytes[f + 12..f + 15].fill(0);
    }
    let error = Animation::parse(bytes.into_boxed_slice())
        .err()
        .expect("an animation must have a display interval");
    assert!(format!("{error:#}").contains("positive"));
    Ok(())
}

#[test]
fn tagged_animation_converts_rgb_and_preserves_coverage() -> Result<()> {
    // Give the flipping decal the linear ICC profile from its still counterpart.
    let mut bytes = fs::read(fixture("animated.webp"))?;
    let still = fs::read(fixture("linear-rgb.webp"))?;
    let at = still.windows(4).position(|b| b == b"ICCP").unwrap();
    let len = u32::from_le_bytes(still[at + 4..at + 8].try_into()?) as usize;
    bytes.splice(30..30, still[at..at + 8 + len + (len & 1)].iter().copied());
    bytes[20] |= 0x20;
    let total = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&total.to_le_bytes());
    let anim = Animation::parse(bytes.into_boxed_slice())?;
    let mut cursor = anim.cursor()?;
    let pixels = cursor.canvas(0)?;
    let mut png = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(fixture(
        "linear-rgb.png",
    ))?))
    .read_info()?;
    let mut reference = vec![0; png.output_buffer_size().unwrap()];
    png.next_frame(&mut reference)?;
    for (actual, expected) in pixels.chunks_exact(4).zip(reference.chunks_exact(4)) {
        assert_eq!(actual[3], expected[3]);
        if expected[3] > 0 {
            for i in 0..3 {
                assert!(actual[i].abs_diff(expected[i]) <= 1);
            }
        }
    }
    Ok(())
}

#[test]
fn rejects_coded_plane_bounds_and_wrong_authored_role() -> Result<()> {
    let mut bytes = fs::read(fixture("patches.webp"))?;
    let plane = bytes.windows(4).rposition(|b| b == b"VP8L").unwrap() + 9;
    bytes[plane] += 1; // Advertise a 3 × 2 image inside a 2 × 2 rectangle.
    let error = Animation::parse(bytes.into_boxed_slice())
        .err()
        .expect("plane should fail");
    assert!(format!("{error:#}").contains("rectangle"));
    let error = Animation::parse(fs::read(fixture("decal.webp"))?.into_boxed_slice())
        .err()
        .expect("still should fail");
    assert!(format!("{error:#}").contains("<image>"));
    Ok(())
}
