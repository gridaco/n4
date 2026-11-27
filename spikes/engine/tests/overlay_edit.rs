use anyhow::Result;
use n4_engine::{Engine, Project, preview::Filmstrip};
use serde_json::json;
use std::{fs, path::Path};

/// A roll changes both adjoining trims. The outgoing take gains two pictures
/// and the incoming take starts two source frames later, keeping its sync.
#[test]
fn rolling_a_cut_and_moving_a_nested_lower_third_preserve_independent_clocks() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let details = root.join("sequences");
    fs::create_dir(&details)?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let take_a = fixtures.join("creator/take-a.mp4");
    let take_b = fixtures.join("creator/take-b.mp4");
    fs::write(
        details.join("speaker-a.json"),
        serde_json::to_vec(&json!({"source":take_a, "source_in":3}))?,
    )?;
    fs::write(
        details.join("lower-third.json"),
        serde_json::to_vec(&json!({"graphics":[{
            "name":"Speaker credit", "channels":{"x":[{"frame":0,"value":4},{"frame":5,"value":14}]},
            "position":[0,34], "content":{"type":"transform_group","children":[
                {"content":{"type":"path","d":"M0 0H32V16H0Z","fill":[255,0,255,255]}},
                {"position":[4,2],"content":{"type":"label","text":"Kim","size":8,"width":24,
                    "font":fixtures.join("fonts/N4GridSans.ttf"),"rgb":[0,0,0]}}
            ]}
        }]}))?,
    )?;

    // A separate, untrimmed edit supplies the expected media pixels at each
    // source ordinal. It has no nested sequences or graphics placements.
    let mut rushes = Engine::new(serde_json::from_value(json!({
        "size":[96,54],"fps":[30,1],"shots":[
            {"source":take_a,"duration":64},{"source":take_b,"duration":64}
        ]
    }))?)?;
    let mut previous_at_cut = Vec::new();
    for (cut, placement) in [(5, 2), (7, 2), (7, 4)] {
        let end = placement + 6;
        fs::write(
            details.join("speaker-b.json"),
            serde_json::to_vec(&json!({"source":take_b, "source_in":20+cut-5}))?,
        )?;
        fs::write(
            root.join("project.proj"),
            format!(
                "project interview fps 30/1 size 96x54\n[0,4) V1 speaker-a\n[4,16) V1 conversation\n  [0,{cut}) V1 speaker-a\n  [{placement},{end}) V2 lower-third\n  [{cut},12) V1 speaker-b\n"
            ),
        )?;
        let edit = Project::load(root)?;
        // Equal dimensions let us verify every filmstrip pixel, including small text.
        let strip = Filmstrip::open(edit.clone(), 96)?;
        let mut picture = Engine::new(edit)?;
        for local in (0..12).rev() {
            let on_a = local < cut;
            let source = if on_a { &take_a } else { &take_b };
            let ordinal = if on_a { 3 + local } else { 20 + local - 5 };
            let original = rushes.render(if on_a { ordinal } else { 64 + ordinal })?;
            let frame = picture.render(4 + local)?;
            let thumb = strip.wait(strip.request(4 + local))?;
            let request = &frame.trace.request;
            assert_eq!(request.source.as_ref(), Some(source));
            assert_eq!(request.source_frame, Some(ordinal));
            assert!(thumb.rgba == frame.rgba, "filmstrip at local {local}");

            if (placement..end).contains(&local) {
                let age = local - placement;
                assert_eq!(request.overlays.len(), 1);
                assert_eq!(request.overlays[0].local_frame, age);
                assert!(thumb.source.is_none());
                assert_eq!(thumb.source_frame, if on_a { local } else { local - cut });
                let x = (4 + 2 * age) as usize;
                // The panel is opaque. Check both edges and the revealed plate
                // on the same row, away from the lettering.
                let row = 35 * 96 * 4;
                for column in 0..96 {
                    let i = row + column * 4;
                    let expected = if (x..x + 32).contains(&column) {
                        &[255, 0, 255, 255][..]
                    } else {
                        &original.rgba[i..i + 4]
                    };
                    assert_eq!(
                        &frame.rgba[i..i + 4],
                        expected,
                        "panel age {age}, x {column}"
                    );
                }
                assert!(
                    frame.rgba[..row - 96 * 4] == original.rgba[..row - 96 * 4],
                    "unobscured picture above the credit at local {local}"
                );
            } else {
                assert!(request.overlays.is_empty());
                assert_eq!(thumb.source.as_ref(), Some(source));
                assert_eq!(thumb.source_frame, ordinal);
                assert!(
                    frame.rgba == original.rgba,
                    "uncovered plate at local {local}"
                );
            }
            if local == 5 && placement == 2 {
                // The first picture moved by the roll switches to the other
                // plate, while the moving credit above it keeps its phase.
                let top = &original.rgba[..96 * 30 * 4];
                if cut == 5 {
                    previous_at_cut = top.to_vec();
                } else {
                    assert!(
                        previous_at_cut != top,
                        "the two takes must be distinguishable"
                    );
                }
            }
        }
    }
    Ok(())
}
