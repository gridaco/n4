use anyhow::{Context, Result, ensure};
use n4_engine::{
    Engine, Project,
    export::write_mp4,
    preview::{Filmstrip, Preview},
    proxy::ProxyMap,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/screen-capture")
}

fn mapping(project: &Project, proxy: PathBuf) -> ProxyMap {
    ProxyMap::from([(
        project.shots[0].source.clone().expect("fixture has media"),
        proxy,
    )])
}

#[test]
fn mixed_filmstrip_samples_native_art_at_the_cut_and_at_shot_local_time() -> Result<()> {
    let media = fixtures().join("../av-cut/scope.mkv");
    let mark = |rgb: [u8; 3], span| {
        json!({
            "span": span,
            "content":{"type":"path", "d":"M32 12 H64 V36 H32 Z", "fill":[rgb[0],rgb[1],rgb[2],255]}
        })
    };
    let project: Project = serde_json::from_value(json!({
        "size":[96,54],"fps":[30,1],
        "shots":[
            {"source":media, "source_in":12, "duration":1},
            {"name":"Wing","duration":2, "background":{"color":[0,0,0]},
             "graphics":[mark([255,0,0],[0,2])]},
            {"name":"Wing detail","duration":2, "background":{"color":[0,0,0]},
             "graphics":[mark([0,255,0],[0,1]), mark([0,0,255],[1,2])]}
        ]
    }))?;
    let strip = Filmstrip::open(project, 48)?;
    for (frame, local, rgb) in [
        (4, 1, [0, 0, 255]),
        (2, 1, [255, 0, 0]),
        (3, 0, [0, 255, 0]),
        (1, 0, [255, 0, 0]),
    ] {
        let thumb = strip.wait(strip.request(frame))?;
        assert_eq!(thumb.size, [48, 27]);
        assert_eq!(thumb.source, None);
        assert_eq!(thumb.source_frame, local);
        assert_eq!(&thumb.rgba[0..4], &[0, 0, 0, 255]);
        let center = 4 * (12 * 48 + 24);
        assert_eq!(&thumb.rgba[center..center + 3], &rgb);
    }
    let media_thumbnail = strip.wait(strip.request(0))?;
    assert_eq!(media_thumbnail.source, Some(media));
    assert_eq!(media_thumbnail.source_frame, 12);
    Ok(())
}

/// The authored sample page paints twelve white/black cells in its footer.
fn barcode(bytes: &[u8], row_width: u32, capture_width: u32) -> u32 {
    (0..12)
        .map(|bit| {
            let x = (37 + bit * 18) * capture_width / 640;
            let y = 326 * capture_width / 640;
            u32::from(bytes[((y * row_width + x) * 4) as usize] > 127) << bit
        })
        .sum()
}

fn stream_hash(raw: &Path) -> Result<String> {
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "640x360",
            "-i",
        ])
        .arg(raw)
        .args([
            "-c:v",
            "copy",
            "-f",
            "streamhash",
            "-hash",
            "sha256",
            "pipe:1",
        ])
        .output()?;
    ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(std::str::from_utf8(&out.stdout)?
        .trim()
        .strip_prefix("0,v,SHA256=")
        .context("missing stream hash")?
        .into())
}

#[test]
fn draft_seek_and_thumbnails_keep_original_ids_and_export_reopens_originals() -> Result<()> {
    let dir = fixtures();
    let mut edit = Project::load(&dir.join("seek.json"))?;
    let original = edit.shots[0].source.clone();
    let proxy = dir.join("draft.mp4");
    let routes = mapping(&edit, proxy.clone());
    let picture = Preview::open_with_proxies(edit.clone(), routes.clone())?;
    let precise = Preview::open(edit.clone())?;
    let strip = Filmstrip::open_with_proxies(edit.clone(), 320, routes.clone())?;
    let original_strip = Filmstrip::open(edit.clone(), 320)?;
    let capture: Value = serde_json::from_slice(&fs::read(dir.join("capture.json"))?)?;
    let records = capture["pictures"]
        .as_array()
        .context("missing browser capture clock")?;
    // Seek backward to 113 after a distant read. This VFR picture's PTS falls
    // after the next key packet's DTS and just before its display PTS.
    let requests = [0, 899, 113, 900, 901, 1619, 899, 1, 1600, 32];
    let mut checked = Vec::new();
    for timeline in requests {
        let tick = if timeline < 900 {
            237 + timeline
        } else {
            3620 + timeline - 900
        };
        let ordinal =
            records.partition_point(|r| r[0].as_u64().unwrap() * 30 <= u64::from(tick) * 1000) - 1;
        let expected = records[ordinal][1].as_u64().unwrap() as u32;
        let draft = picture.wait(picture.request(timeline))?;
        let full = precise.wait(precise.request(timeline))?;
        assert_eq!(draft.trace.proxy.as_ref(), Some(&proxy));
        assert_eq!(full.trace.proxy, None);
        assert_ne!(draft.rgba, full.rgba);
        for frame in [&draft, &full] {
            assert_eq!(frame.trace.request.source, original);
            assert_eq!(frame.trace.request.source_frame, Some(ordinal as u32));
            assert_eq!(barcode(&frame.rgba, 640, 640), expected);
        }
        let low = strip.wait(strip.request(timeline))?;
        let high = original_strip.wait(original_strip.request(timeline))?;
        assert_eq!(low.proxy.as_ref(), Some(&proxy));
        assert_eq!(high.proxy, None);
        for thumbnail in [low, high] {
            assert_eq!(thumbnail.source, original);
            assert_eq!(thumbnail.source_frame, ordinal as u32);
            assert_eq!(barcode(&thumbnail.rgba, 320, 320), expected);
        }
        checked.push([timeline, ordinal as u32, expected]);
    }
    // Keep draft playback active while a separate full-quality engine exports both trims.
    edit.shots[0].duration = 4;
    edit.shots[1].duration = 4;
    let tmp = tempfile::tempdir()?;
    let full_raw = tmp.path().join("full.rgba");
    let proxy_raw = tmp.path().join("draft.rgba");
    let mut full_file = fs::File::create_new(&full_raw)?;
    let mut draft_file = fs::File::create_new(&proxy_raw)?;
    for time in [0, 1, 2, 3, 900, 901, 902, 903] {
        full_file.write_all(&precise.wait(precise.request(time))?.rgba)?;
        draft_file.write_all(&picture.wait(picture.request(time))?.rgba)?;
    }
    drop((full_file, draft_file));
    let export = write_mp4(&mut Engine::new(edit)?, &tmp.path().join("clean.mp4"))?;
    let original_digest = stream_hash(&full_raw)?;
    let draft_digest = stream_hash(&proxy_raw)?;
    assert_eq!(export.rgba_sha256, original_digest);
    assert_ne!(export.rgba_sha256, draft_digest);
    println!(
        "PROXY_RESULT {}",
        json!({"requests_ordinal_and_barcode":checked,"filmstrip_width":320,
        "capture_size":[640,360],"proxy_size":[320,180],"export_frames":export.frames,
        "full_inspection_rgba_sha256":original_digest,"draft_preview_rgba_sha256":draft_digest,
        "encoder_input_rgba_sha256":export.rgba_sha256})
    );
    Ok(())
}

#[test]
fn inset_only_proxy_preserves_primary_filmstrip_and_export_uses_original_inset() -> Result<()> {
    let dir = fixtures();
    let mut edit = Project::load(&dir.join("seek.json"))?;
    let capture = edit.shots[0].source.clone().context("fixture has media")?;
    let base = dir.join("../av-cut/scope.mkv");
    let proxy = dir.join("draft.mp4");
    edit.shots.truncate(1);
    edit.shots[0].source = Some(base.clone());
    edit.shots[0].source_in = 12;
    edit.shots[0].duration = 4;
    edit.graphics = serde_json::from_value(json!([{
        "content": {"type":"video", "source":capture, "source_in":237,
                    "start":1, "duration":2, "size":[320,180]}
    }]))?;
    let routes = ProxyMap::from([(capture.clone(), proxy.clone())]);
    let preview = Preview::open_with_proxies(edit.clone(), routes.clone())?;
    let inspect = Preview::open(edit.clone())?;
    let strip = Filmstrip::open_with_proxies(edit.clone(), 320, routes)?;
    let measurements: Value = serde_json::from_slice(&fs::read(dir.join("capture.json"))?)?;
    let records = measurements["pictures"]
        .as_array()
        .context("missing recorded source PTS")?;
    let tmp = tempfile::tempdir()?;
    let full_raw = tmp.path().join("original.rgba");
    let mut full_file = fs::File::create_new(&full_raw)?;
    for frame in 0..4 {
        let draft = preview.wait(preview.request(frame))?;
        let full = inspect.wait(inspect.request(frame))?;
        full_file.write_all(&full.rgba)?;
        assert_eq!(draft.trace.proxy, None);
        assert_eq!(draft.trace.request.source.as_ref(), Some(&base));
        let thumb = strip.wait(strip.request(frame))?;
        assert_eq!(thumb.source.as_ref(), Some(&base));
        assert_eq!(thumb.source_frame, 12 + frame);
        assert_eq!(thumb.proxy, None);
        if (1..3).contains(&frame) {
            let tick = u64::from(237 + frame - 1);
            let ordinal =
                records.partition_point(|r| r[0].as_u64().unwrap() * 30 <= tick * 1000) - 1;
            let expected_id = records[ordinal][1].as_u64().unwrap() as u32;
            for (picture, route) in [(&draft, Some(&proxy)), (&full, None)] {
                let layers = &picture.trace.request.video_layers;
                assert_eq!(layers.len(), 1);
                assert_eq!(layers[0].source, capture);
                assert_eq!(layers[0].source_frame, ordinal as u32);
                assert_eq!(layers[0].proxy.as_ref(), route);
                assert_eq!(barcode(&picture.rgba, 640, 320), expected_id);
            }
            ensure!(
                draft.rgba != full.rgba,
                "inset proxy made no visible change"
            );
            const ROW_BYTES: usize = 640 * 4;
            let draft_rows = draft.rgba.as_chunks::<ROW_BYTES>().0;
            let full_rows = full.rgba.as_chunks::<ROW_BYTES>().0;
            for (y, (a, b)) in draft_rows.iter().zip(full_rows).enumerate() {
                let after_inset = if y < 180 { 320 * 4 } else { 0 };
                ensure!(
                    a[after_inset..] == b[after_inset..],
                    "proxy changed the primary picture"
                );
            }
        } else {
            assert!(draft.trace.request.video_layers.is_empty());
            ensure!(
                draft.rgba == full.rgba,
                "proxy changed a frame outside the inset placement"
            );
        }
    }
    drop(full_file);
    let exported = write_mp4(&mut Engine::new(edit)?, &tmp.path().join("export.mp4"))?;
    assert_eq!(exported.rgba_sha256, stream_hash(&full_raw)?);
    Ok(())
}

#[test]
fn derivative_with_one_shifted_onset_is_rejected() -> Result<()> {
    let dir = fixtures();
    let tmp = tempfile::tempdir()?;
    let bad = tmp.path().join("bad.mp4");
    let generated = Command::new("python3")
        .arg("-B")
        .arg(dir.join("make_proxy.py"))
        .arg(dir.join("recording.mp4"))
        .arg(&bad)
        .arg("--shift-one-onset")
        .output()?;
    ensure!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let edit = Project::load(&dir.join("seek.json"))?;
    let routes = mapping(&edit, bad);
    let error = match Filmstrip::open_with_proxies(edit, 320, routes) {
        Ok(_) => anyhow::bail!("accepted a proxy with a one-millisecond timing error"),
        Err(err) => format!("{err:#}"),
    };
    assert!(
        error.contains("120"),
        "expected mismatched ordinal in: {error}"
    );
    Ok(())
}
