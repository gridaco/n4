use super::*;
use std::fs;

#[test]
fn placement_thumbnail_can_round_either_scaled_axis() -> Result<()> {
    // At 120 pixels on the long side a 9:16 image is 67.5 pixels wide.
    check_aspect_ratio([90, 160], [68, 120])?;
    check_aspect_ratio([160, 90], [120, 68])?;
    assert!(check_aspect_ratio([90, 160], [69, 120]).is_err());
    assert!(check_aspect_ratio([160, 90], [120, 69]).is_err());
    Ok(())
}

#[test]
fn animation_clock_keeps_exact_intervals_and_skips_zero_exposures() -> Result<()> {
    let timeline = AnimationTimeline::new(&[0, 40, 0, 80, 20, 0])?;
    assert_eq!(timeline.encoded_ordinals, [1, 3, 4]);
    let fifty = timeline.at_rate(FrameRate(50, 1))?;
    let displayed = (0..fifty.ticks)
        .map(|tick| fifty.resolve(tick))
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(displayed, [0, 0, 1, 1, 1, 1, 2]);
    assert!(fifty.resolve(7).is_err());
    let film = timeline.at_rate(FrameRate(24_000, 1001))?;
    assert_eq!(film.ticks, 4);
    assert_eq!(film.resolve(1)?, 1); // 41.708 ms
    assert_eq!(film.resolve(3)?, 2); // 125.125 ms
    assert!(AnimationTimeline::new(&[0, 0]).is_err());
    // A movie index could infer 30fps from 33/34ms PTS. These WebP holds
    // explicitly start B at 34ms, so a 30fps tick at 33 1/3ms still shows A.
    let timed = AnimationTimeline::new(&[34, 33, 33])?.at_rate(FrameRate(30, 1))?;
    assert_eq!(timed.resolve(1)?, 0);
    Ok(())
}

#[test]
fn transparent_thumbnail_charges_full_canvas_bytes_and_preserves_source_pixels() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/webp/patches.webp");
    let animation = animation::Animation::open(&path)?.context("expected animation")?;
    let charge = animation.cursor_bytes();
    let source = IndexedVideo::from_animation(&path, 2, 1, FrameRate(50, 1), animation)?;
    let mut reader = VideoReader::with_limits(charge + 8, 0);
    for (tick, expected) in [
        (6, [255, 255, 255, 255, 0, 255, 0, 255]),
        (1, [255, 0, 0, 255, 0, 0, 0, 0]),
        (2, [0, 0, 0, 0, 0, 255, 0, 255]),
        (0, [255, 0, 0, 255, 0, 0, 0, 0]),
    ] {
        let (frame, hit) = reader.read(&source, source.resolve(tick)?)?;
        assert!(!hit);
        assert_eq!(frame.alpha, FrameAlpha::Straight);
        assert_eq!(frame.rgba.as_slice(), &expected);
        assert_eq!(reader.cursors[0].retained_bytes, charge);
        assert_eq!(reader.bytes, 8);
    }
    assert!(reader.read(&source, 0)?.1);
    let mut small = VideoReader::with_limits(8, 0);
    let (tile, _) = small.read(&source, 1)?;
    assert_eq!(tile.rgba.as_slice(), &[0, 0, 0, 0, 0, 255, 0, 255]);
    assert!(small.cursors.is_empty());
    assert_eq!(small.bytes, 8);
    assert!(small.read(&source, 1)?.1);
    assert!(source.check_proxy_timeline(&source).is_err());
    Ok(())
}

#[test]
fn gif_and_webp_cursors_share_one_thumbnail_budget() -> Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let fps = FrameRate(50, 1);
    let size = [2, 1];
    let make = |name: &str| -> Result<_> {
        let path = fixtures.join(name);
        let anim = animation::Animation::open(&path)?.context("expected timed image")?;
        let charge = anim.cursor_bytes();
        let source = IndexedVideo::from_animation(&path, size[0], size[1], fps, anim)?;
        Ok((source, charge))
    };
    let (gif, gif_bytes) = make("gif/restore.gif")?;
    let (webp, webp_bytes) = make("webp/patches.webp")?;
    let mut reader = VideoReader::with_limits(gif_bytes.max(webp_bytes) + 8, 0);
    reader.begin_frame([&gif, &webp]);

    // Switching codecs must release the previously parked canvas before
    // decoding, even when both sources are visible at this output size.
    for (source, charge, tick) in [
        (&gif, gif_bytes, 0),
        (&webp, webp_bytes, 0),
        (&gif, gif_bytes, 6),
    ] {
        let (frame, hit) = reader.read(source, source.resolve(tick)?)?;
        assert!(!hit);
        assert_eq!(frame.alpha, FrameAlpha::Straight);
        assert_eq!(frame.rgba.as_slice(), &[255, 0, 0, 255, 0, 0, 0, 0]);
        assert_eq!(reader.cursors.len(), 1);
        assert_eq!(reader.cursors[0].retained_bytes, charge);
        assert!(reader.bytes + charge <= reader.limit);
        assert!(reader.read(source, source.resolve(tick + 1)?)?.1);
    }
    Ok(())
}

/// Starts with an empty frame cache and requests every displayed source
/// picture once, in descending order. Excludes GPU and device presentation.
#[test]
#[ignore = "manual cold reverse decode with a supplied animated image"]
fn measure_animation_reverse_reads() -> Result<()> {
    use std::time::Instant;
    let path = std::env::var_os("N4_ANIMATION_SAMPLE").context("set N4_ANIMATION_SAMPLE")?;
    let path = Path::new(&path);
    let start = Instant::now();
    let animation = animation::Animation::open(path)?.context("expected animated image footage")?;
    let [w, h] = animation.size();
    let source = IndexedVideo::from_animation(path, w, h, FrameRate(30, 1), animation)?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.;
    let count = u32::try_from(source.index.pts.len())?;
    let mut reader = VideoReader::default();
    reader.begin_frame([&source]);
    let mut latencies = Vec::new();
    let mut peak_retained = 0;
    for ordinal in (0..count).rev() {
        let start = Instant::now();
        let (pixels, hit) = reader.read(&source, ordinal)?;
        std::hint::black_box(pixels);
        latencies.push(start.elapsed().as_micros());
        assert!(!hit);
        let canvases: usize = reader
            .cursors
            .iter()
            .map(|cursor| cursor.retained_bytes)
            .sum();
        peak_retained = peak_retained.max(reader.bytes + canvases);
    }
    let Some(Cursor::Animation(cursor)) = reader.cursors.front().map(|c| &c.decoder) else {
        anyhow::bail!("benchmark requires a canvas that fits in one default reader");
    };
    latencies.sort_unstable();
    let p95 = latencies[(latencies.len() * 95).div_ceil(100) - 1];
    println!(
        "size={w}x{h} display_frames={count} open_ms={open_ms:.2} reverse_p95_ms={:.2} decodes_in_reverse={} peak_cached_rgba_and_canvas_bytes={peak_retained}",
        p95 as f64 / 1000.,
        cursor.frames_decoded()
    );
    Ok(())
}

#[test]
fn accepted_bytes_survive_a_damaged_later_frame() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("mislabeled.png");
    let mut bytes =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/webp/patches.webp"))?;
    fs::write(&path, &bytes)?;
    let asset = animation::Animation::open(&path)?.unwrap();
    let digest = asset.digest();
    let final_bitstream = bytes.windows(4).rposition(|b| b == b"VP8L").unwrap();
    bytes[final_bitstream + 13..].fill(0xff); // Dimensions, RIFF and ANMF headers still parse.
    fs::write(&path, bytes)?;
    let err = animation::Animation::open(&path)
        .err()
        .expect("corrupt replacement rejected");
    assert!(format!("{err:#}").contains("mislabeled.png"));
    assert_eq!(asset.digest(), digest);
    let mut fresh_cursor = asset.cursor([4, 2])?;
    assert_eq!(&fresh_cursor.read(2)?[..4], &[255; 4]);
    Ok(())
}

#[test]
fn resizing_excludes_hidden_color_and_mixes_light() -> Result<()> {
    assert_eq!(
        animation::resize(&[255, 0, 0, 255, 0, 255, 0, 0], [2, 1], [1, 1])?,
        [255, 0, 0, 128]
    );
    assert_eq!(
        animation::resize(&[0, 0, 0, 255, 255, 255, 255, 255], [2, 1], [1, 1])?,
        [188, 188, 188, 255]
    );
    Ok(())
}
