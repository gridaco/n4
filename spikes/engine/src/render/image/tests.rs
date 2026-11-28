use super::{PreparedImage, dimensions, paint_image, prepare_image};
use anyhow::Result;
use std::fs;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/webp")
        .join(name)
}

fn reference_rgba(name: &str) -> Result<Vec<u8>> {
    let mut reader =
        png::Decoder::new(BufReader::new(fs::File::open(fixture(name))?)).read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels)?;
    assert_eq!((info.width, info.height), (24, 16));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    Ok(pixels)
}

#[test]
fn pixels_match_original_art_libwebp_and_littlecms() -> Result<()> {
    for (name, tolerance) in [
        ("decal", 0),
        ("opaque-lossy", 1),
        ("alpha-lossy", 1),
        ("linear-rgb", 1),
    ] {
        let path = fixture(&format!("{name}.webp"));
        assert_eq!(dimensions(&path)?, [24, 16]);
        let raster = paint_image(&path)?;
        assert_eq!(raster.size, [24, 16]);
        assert_eq!(raster.origin, [0, 0]);
        let expected = reference_rgba(&format!("{name}.png"))?;
        assert_eq!(raster.rgba.len(), expected.len());
        for (index, (&got, want)) in raster.rgba.iter().zip(expected).enumerate() {
            let allowed = if index % 4 == 3 { 0 } else { tolerance };
            assert!(
                got.abs_diff(want) <= allowed,
                "{name} byte {index}: decoded {got}, expected {want} ±{allowed}"
            );
        }
    }
    Ok(())
}

#[test]
fn lossless_webp_uses_pixel_alpha_even_when_the_header_hint_is_clear() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("art.webp");
    for name in ["decal", "linear-rgb"] {
        let mut bytes = fs::read(fixture(&format!("{name}.webp")))?;
        fs::write(&path, &bytes)?;
        let expected = paint_image(&path)?.rgba;
        assert!(expected.chunks_exact(4).any(|p| (1..255).contains(&p[3])));
        let lossless = bytes.windows(4).position(|v| v == b"VP8L").unwrap();
        // alpha_is_used is bit 28 of the lossless dimensions word after 0x2f.
        bytes[lossless + 12] &= !0x10;
        if &bytes[12..16] == b"VP8X" {
            bytes[20] &= !0x10;
        }
        fs::write(&path, &bytes)?;
        assert_eq!(paint_image(&path)?.rgba, expected, "{name}");
        assert_eq!(fs::read(&path)?, bytes);
    }
    Ok(())
}

#[test]
fn webp_reader_follows_bytes_even_under_another_extension() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let renamed = temp.path().join("art.png");
    fs::copy(fixture("decal.webp"), &renamed)?;
    let PreparedImage::Pinned(image) = prepare_image(&renamed)? else {
        panic!("recognized WebP should have an accepted raster")
    };
    assert_eq!(image.rgba, reference_rgba("decal.png")?);
    fs::copy(fixture("decal.png"), &renamed)?;
    assert!(matches!(
        prepare_image(&renamed)?,
        PreparedImage::IntrinsicSize([24, 16])
    ));
    Ok(())
}

#[test]
fn gif_recognition_uses_image_count_and_pins_the_displayed_canvas() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let renamed = temp.path().join("sticker.bin");
    let animation = include_bytes!("../../../fixtures/gif/restore.gif");
    fs::write(&renamed, animation)?;
    let error = prepare_image(&renamed)
        .err()
        .expect("multi-image GIF on a still layer");
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("sticker.bin") && diagnostic.contains("<video>"),
        "{diagnostic}"
    );

    // Keep the infinite-loop extension and the first partial image only.
    let second_control = animation
        .windows(3)
        .enumerate()
        .filter(|(_, block)| *block == b"\x21\xf9\x04")
        .nth(1)
        .expect("fixture has a second graphic control")
        .0;
    let mut single = animation[..second_control].to_vec();
    single.push(b';');
    fs::write(&renamed, single)?;
    assert_eq!(dimensions(&renamed)?, [8, 4]);
    let PreparedImage::Pinned(still) = prepare_image(&renamed)? else {
        panic!("recognized single-image GIF must pin its accepted pixels");
    };
    assert_eq!(still.size, [8, 4]);
    let row = [[255, 0, 0, 255]; 4].concat();
    let expected = [row, vec![0; 16]].concat().repeat(4);
    assert_eq!(still.rgba, expected);
    assert_eq!(paint_image(&renamed)?.rgba, expected);
    Ok(())
}

#[test]
fn rejection_identifies_animated_input_and_malformed_icc() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let animated = temp.path().join("two-frames.bin");
    fs::copy(fixture("animated.webp"), &animated)?;
    let error = prepare_image(&animated).err().expect("animation must fail");
    let message = format!("{error:#}").to_lowercase();
    assert!(
        message.contains("two-frames.bin") && message.contains("animat"),
        "{message}"
    );

    let path = temp.path().join("bad-profile.bin");
    let mut bytes = fs::read(fixture("linear-rgb.webp"))?;
    let signature = bytes.windows(4).position(|v| v == b"acsp").unwrap();
    bytes[signature..signature + 4].fill(0);
    fs::write(&path, bytes)?;
    let error = prepare_image(&path)
        .err()
        .expect("invalid profile must fail");
    let message = format!("{error:#}").to_lowercase();
    assert!(
        message.contains("bad-profile.bin") && message.contains("icc"),
        "{message}"
    );
    Ok(())
}

#[test]
fn validate_canvas_and_payload_before_accepting_a_webp() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("candidate.webp");
    let mut bytes = fs::read(fixture("decal.webp"))?;
    assert_eq!(&bytes[12..16], b"VP8L");
    let shape = u32::from_le_bytes(bytes[21..25].try_into()?);
    bytes[21..25].copy_from_slice(&((shape & !0x3fff) | 8192).to_le_bytes());
    fs::write(&path, &bytes)?;
    assert!(dimensions(&path).is_err());
    assert!(prepare_image(&path).is_err());

    // Valid 24×16 VP8L header and RIFF extents, with all compressed sample data removed.
    bytes[21..25].copy_from_slice(&shape.to_le_bytes());
    bytes.truncate(25);
    bytes.push(0);
    bytes[4..8].copy_from_slice(&18u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&5u32.to_le_bytes());
    fs::write(&path, bytes)?;
    let error = prepare_image(&path)
        .err()
        .expect("missing sample data must fail");
    assert!(format!("{error:#}").contains("candidate.webp"));
    Ok(())
}

#[test]
fn rejects_oversized_file_and_profile_before_reading_their_payloads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("oversize.webp");
    let mut header = fs::read(fixture("linear-rgb.webp"))?;
    // Retain RIFF, VP8X, and the ICCP chunk header. The sparse payload tests
    // input accounting independently of the limits on decoded dimensions.
    let profile = header.windows(4).position(|b| b == b"ICCP").unwrap();
    header.truncate(profile + 8);
    for (payload, diagnostic) in [
        (4 * 1024 * 1024 + 2u32, "icc"),
        (256 * 1024 * 1024u32, "encoded"),
    ] {
        let file_len = header.len() as u32 + payload;
        header[4..8].copy_from_slice(&(file_len - 8).to_le_bytes());
        header[profile + 4..].copy_from_slice(&payload.to_le_bytes());
        let mut file = fs::File::create(&path)?;
        file.write_all(&header)?;
        file.set_len(u64::from(file_len))?;
        drop(file);
        let error = prepare_image(&path).err().expect("input exceeds budget");
        assert!(format!("{error:#}").to_lowercase().contains(diagnostic));
    }
    Ok(())
}
