use anyhow::Result;
use n4_engine::{Engine, Frame, FrameRate, Project, Shot, Title};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn source_history_follows_vfr_presentation_ticks_and_restarts_after_a_trimmed_cut() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let shader = dir.path().join("prior.wgsl");
    fs::write(
        &shader,
        "fn temporal(p: vec2i, c: vec4f, n: u32) -> vec4f {
        return vec4f(history_at(p, 2u).rgb, 1.0);
    }",
    )?;
    let source = fixtures().join("numbered-vfr.mp4");
    let footage = serde_json::json!({"content":{"type":"footage",
        "history":{"frames":2,"shader":shader}}});
    let project = serde_json::from_value(serde_json::json!({
        "size":[320,180],"fps":[30,1],"shots":[
            {"source":source,"source_in":4,"duration":7,"graphics":[footage]},
            {"source":source,"source_in":21,"duration":2,"graphics":[footage]}]
    }))?;
    let mut engine = Engine::new(project)?;
    for (tick, history, expected_barcode) in [
        (4, [Some(6), Some(6)], 6),
        (7, [None, None], 0),
        (5, [Some(7), Some(6)], 6),
        (8, [Some(19), None], 0),
        (4, [Some(6), Some(6)], 6),
    ] {
        let frame = engine.render(tick)?;
        assert_eq!(frame.trace.request.source_history, history);
        let barcode = (0..7).fold(0, |id, bit| {
            let red = frame.rgba[4 * (172 * 320 + 8 + 8 * bit)];
            id | (u32::from(red > 128) << bit)
        });
        assert_eq!(barcode, expected_barcode);
    }
    Ok(())
}

fn in_memory_project() -> Project {
    let source = fixtures().join("numbered.mkv");
    Project {
        overlays: vec![],
        graphics: vec![],
        exposure: None,
        audio: vec![],
        size: [641, 360],
        fps: FrameRate(30, 1),
        shots: vec![
            Shot {
                cameras: vec![],
                camera: None,
                camera_cuts: vec![],
                source: Some(source.clone()),
                source_in: 10,
                duration: 30,
                name: None,
                background: None,
                graphics: vec![],
            },
            Shot {
                cameras: vec![],
                camera: None,
                camera_cuts: vec![],
                source: Some(source),
                source_in: 3,
                duration: 30,
                name: None,
                background: None,
                graphics: vec![],
            },
        ],
        title: Title {
            text: "A frame worth keeping".into(),
            backplate: None,
            font: fixtures().join("fonts/N4GridSans.ttf"),
            font_size: 32,
            width: 450,
            position: [32, 150],
            start: 6,
            fade_frames: 6,
        },
    }
}

/// Read the burned-in barcode from the final GPU pixels. This oracle is independent of the plan.
fn check_marker(frame: &Frame, expected: u32) {
    let mut id = 0;
    for bit in 0..7 {
        let x = 16 + 16 * bit;
        let red = frame.rgba[((344 * 641 + x) * 4) as usize];
        assert!(red == 255 || red == 0);
        if red == 255 {
            id |= 1 << bit;
        }
    }
    assert_eq!(id, expected);
}

#[test]
fn two_authoring_routes_cuts_retiming_and_wrap() -> Result<()> {
    let mut parsed = Engine::new(Project::load(&fixtures().join("title.json"))?)?;
    let mut direct = Engine::new(in_memory_project())?;
    for (time, source) in [(0, 10), (29, 39), (30, 3), (59, 32), (15, 25)] {
        let a = parsed.render(time)?;
        let b = direct.render(time)?;
        assert_eq!(a.trace.request, b.trace.request);
        assert_eq!(a.rgba, b.rgba);
        assert_eq!(a.trace.request.source_frame, Some(source));
        check_marker(&a, source);
    }
    assert!(parsed.render(60).is_err());
    let old = parsed.render(15)?;
    assert_eq!(old.trace.paragraph_lines, Some(1));
    parsed.edit_title(Some(12), None)?;
    let revised = parsed.render(15)?;
    assert_eq!(revised.trace.request.title_opacity, [3, 6]);
    assert!(revised.trace.reused_decode && revised.trace.reused_paragraph);
    assert_ne!(revised.rgba, old.rgba);
    check_marker(&revised, 25);

    parsed.edit_title(None, Some(240))?;
    direct.edit_title(Some(12), Some(240))?;
    let narrow = parsed.render(15)?;
    let narrow_direct = direct.render(15)?;
    assert_eq!(narrow.trace.request, narrow_direct.trace.request);
    assert_eq!(narrow.rgba, narrow_direct.rgba);
    assert!(narrow.trace.reused_decode && !narrow.trace.reused_paragraph);
    assert_eq!(narrow.trace.paragraph_lines, Some(2));
    // The second line must actually paint below the old line's 42-pixel box.
    assert!(
        narrow
            .rgba
            .as_chunks::<{ 641 * 4 }>()
            .0
            .iter()
            .zip(revised.rgba.as_chunks::<{ 641 * 4 }>().0.iter())
            .skip(192)
            .take(42)
            .any(|(a, b)| a != b)
    );
    parsed.render(45)?;
    assert_eq!(parsed.render(15)?.rgba, narrow.rgba);
    assert!(parsed.edit_title(Some(0), Some(0)).is_err());
    let preserved = parsed.render(15)?;
    assert_eq!(preserved.trace.revision, narrow.trace.revision);
    assert_eq!(preserved.rgba, narrow.rgba);
    parsed.edit_title(None, Some(450))?;
    assert_eq!(parsed.render(15)?.rgba, revised.rgba);
    Ok(())
}

#[test]
fn backward_seek_matches_new_cli_process_and_failed_batch_stays_private() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let copy = tmp.path().join("자료 with spaces");
    fs::create_dir(&copy)?;
    fs::create_dir(copy.join("fonts"))?;
    for rel in ["title.json", "numbered.mkv", "fonts/N4GridSans.ttf"] {
        fs::copy(fixtures().join(rel), copy.join(rel))?;
    }
    let run = |sequence: &str, dir: &str| -> Result<_> {
        let output = Command::new(env!("CARGO_BIN_EXE_n4"))
            .arg("render")
            .arg(copy.join("title.json"))
            .args([
                "--frames",
                sequence,
                "--title-start",
                "12",
                "--title-width",
                "240",
                "--out",
            ])
            .arg(tmp.path().join(dir))
            .output()?;
        Ok(output)
    };
    for (sequence, dir) in [("15,45,15", "scrub"), ("15", "fresh")] {
        let result = run(sequence, dir)?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let result = |p: &str| fs::read(tmp.path().join(p));
    assert_eq!(
        result("scrub/002-f0015.png")?,
        result("fresh/000-f0015.png")?
    );
    assert_eq!(
        result("scrub/000-f0015.png")?,
        result("fresh/000-f0015.png")?
    );
    assert_ne!(
        result("scrub/001-f0045.png")?,
        result("fresh/000-f0015.png")?
    );
    assert!(!run("15,60", "bad")?.status.success());
    assert!(!tmp.path().join("bad").exists());
    Ok(())
}
