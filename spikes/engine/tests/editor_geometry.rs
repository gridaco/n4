#![cfg(feature = "host-audio")]

use anyhow::{Context, Result};
use n4_engine::{
    Frame,
    editor::{EditorCommand, EditorError, EditorSession, LayerRef, Visibility},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const GREEN: [u8; 4] = [0, 255, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

fn example() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/signal-atlas")
}

fn pixel(picture: &Frame, [x, y]: [u32; 2]) -> [u8; 4] {
    let offset = (4 * (y * picture.size[0] + x)) as usize;
    picture.rgba[offset..offset + 4].try_into().unwrap()
}

fn center(point: [u32; 2]) -> [f64; 2] {
    point.map(|coordinate| f64::from(coordinate) + 0.5)
}

fn pick(
    session: &mut EditorSession,
    revision: u64,
    frame: u32,
    point: [u32; 2],
) -> Result<Value, EditorError> {
    session.dispatch(EditorCommand::HitTest {
        revision,
        frame,
        point: center(point),
    })
}

#[test]
fn clipped_raster_and_editable_title_match_their_rendered_pick_areas() -> Result<()> {
    let mut session = EditorSession::open(&example())?;
    let frame = 42;
    let plate = LayerRef {
        occurrence: "V1:survey@0".into(),
        layers: vec![0, 0],
    };
    let picture = session.render(0, frame)?;
    let center_point = [480, 200];
    let rounded_corner = [354, 80];
    assert_ne!(
        pixel(&picture, center_point),
        pixel(&picture, rounded_corner)
    );
    assert_eq!(
        pick(&mut session, 0, frame, center_point)?["hits"][0],
        json!(plate)
    );
    assert_eq!(
        pick(&mut session, 0, frame, rounded_corner)?["hits"],
        json!([])
    );
    session.dispatch(EditorCommand::Seek { frame })?;
    session.dispatch(EditorCommand::SelectAt {
        revision: 0,
        frame,
        point: center(center_point),
    })?;
    let outline = session.state().hud.context("the relief has no outline")?;
    assert_eq!(outline.target, plate);
    assert_eq!(outline.visibility, Visibility::Visible);
    assert_eq!(outline.local_bounds, Some([0., 0., 568., 380.]));
    assert_eq!(outline.clips.len(), 1);
    let glyph = (170..300)
        .flat_map(|y| (38..340).map(move |x| [x, y]))
        .find(|&point| pixel(&picture, point)[0] < 45)
        .context("no visible title ink")?;
    let title = LayerRef {
        occurrence: "V1:survey@0".into(),
        layers: vec![2],
    };
    assert_eq!(
        pick(&mut session, 0, frame, glyph)?["hits"][0],
        json!(title)
    );
    session.dispatch(EditorCommand::SelectAt {
        revision: 0,
        frame,
        point: center(glyph),
    })?;
    let outline = session.state().hud.context("the title has no outline")?;
    assert_eq!(outline.target, title);
    assert_eq!(outline.visibility, Visibility::Visible);
    assert!(outline.composition_quad.is_some());
    Ok(())
}

#[test]
fn image_click_area_uses_the_accepted_size_regardless_of_pixel_alpha() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    let sequences = root.join("sequences");
    fs::create_dir(&sequences)?;
    let source = root.join("sample.png");
    let write_image = |width: u32| -> Result<()> {
        let mut encoder = png::Encoder::new(fs::File::create(&source)?, width, 6);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()?
            .write_image_data(&vec![0; (width * 6 * 4) as usize])?;
        Ok(())
    };
    write_image(8)?;
    fs::write(
        root.join("project.proj"),
        "project image fps 30/1 size 40x30\n[0,4) V1 shot\n",
    )?;
    fs::write(
        sequences.join("shot.xml"),
        r##"<sequence background="#000000">
          <rect width="40" height="30" fill="#ff0000"/>
          <image source="sample.png" position="10 8"/>
        </sequence>"##,
    )?;
    let mut session = EditorSession::open(root)?;
    let image = json!({"occurrence":"V1:shot@0","layers":[1]});
    let back = json!({"occurrence":"V1:shot@0","layers":[0]});
    let inside = [16, 12];
    let outside = [23, 12];
    let picture = session.render(0, 0)?;
    assert_eq!(pixel(&picture, inside), RED);
    assert_eq!(
        pick(&mut session, 0, 0, inside)?["hits"],
        json!([image, back]),
    );
    assert_eq!(pick(&mut session, 0, 0, outside)?["hits"], json!([back]));
    write_image(20)?;
    assert_eq!(pick(&mut session, 0, 0, outside)?["hits"], json!([back]));
    session.dispatch(EditorCommand::Reload {
        revision: 0,
        discard_changes: false,
    })?;
    assert_eq!(
        pick(&mut session, 1, 0, outside)?["hits"],
        json!([image, back]),
    );
    Ok(())
}

#[test]
#[ignore = "post-alpha: the picker currently treats RGB grade as unresolved coverage"]
fn rgb_grade_preserves_canvas_pick_area() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    let sequences = root.join("sequences");
    fs::create_dir(&sequences)?;
    fs::write(
        root.join("project.proj"),
        "project grade fps 30/1 size 40x30\n[0,4) V1 shot\n",
    )?;
    fs::write(
        sequences.join("shot.xml"),
        r##"<sequence background="#000000">
          <rect width="40" height="30" fill="#ff0000"/>
          <rect x="10" y="8" width="14" height="10" fill="#ffffff">
            <rgb-grade gain="0 0 1"/>
          </rect>
        </sequence>"##,
    )?;
    let mut session = EditorSession::open(root)?;
    let front = json!({"occurrence":"V1:shot@0","layers":[1]});
    let back = json!({"occurrence":"V1:shot@0","layers":[0]});
    let inside = [16, 12];
    let outside = [28, 12];
    let picture = session.render(0, 0)?;
    assert_eq!(pixel(&picture, inside), [0, 0, 255, 255]);
    assert_eq!(pixel(&picture, outside), RED);
    assert_eq!(pick(&mut session, 0, 0, outside)?["hits"], json!([back]));
    let hit = pick(&mut session, 0, 0, inside)?;
    assert_eq!(hit["hits"], json!([front, back]));
    assert_eq!(hit["indeterminate"], false);
    Ok(())
}

#[test]
fn video_picks_follow_placement_time_and_exposed_footage_can_be_selected() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    let sequences = root.join("sequences");
    fs::create_dir(&sequences)?;
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered.mkv"),
        root.join("plate.mkv"),
    )?;
    fs::write(
        root.join("project.proj"),
        "project media fps 30/1 size 64x36\n[0,8) V1 shot\n[1,7) V2 inset\n[8,12) V1 lifted\n",
    )?;
    fs::write(
        sequences.join("shot.xml"),
        r##"<sequence source="plate.mkv">
          <rect x="15" y="15" width="25" height="25" fill="#ff0000"/>
        </sequence>"##,
    )?;
    fs::write(
        sequences.join("inset.xml"),
        r#"<sequence>
          <video source="plate.mkv" size="32 18" position="36 12" rotation="90"
                 start="2" duration="3" source-in="17">
            <mask><rect x="4" y="2" width="12" height="10"/></mask>
          </video>
        </sequence>"#,
    )?;
    fs::write(
        sequences.join("lifted.xml"),
        r#"<sequence source="plate.mkv">
          <footage position="4 6">
            <mask><rect x="4" y="4" width="24" height="15"/></mask>
          </footage>
        </sequence>"#,
    )?;
    let mut session = EditorSession::open(root)?;
    let video = LayerRef {
        occurrence: "V2:inset@1".into(),
        layers: vec![0],
    };
    let red = json!({"occurrence":"V1:shot@0","layers":[0]});
    let video_point = [29, 22];
    let cropped_point = [33, 30];
    for (frame, active) in [(2, false), (3, true), (5, true), (6, false)] {
        let picture = session.render(0, frame)?;
        let geometry = session.dispatch(EditorCommand::LayerGeometry {
            revision: 0,
            frame,
            layer: video.clone(),
        })?;
        assert_eq!(
            geometry["visibility"],
            if active { "visible" } else { "inactive" },
            "frame {frame}",
        );
        if active {
            assert_eq!(geometry["localBounds"], json!([0., 0., 32., 18.]));
            assert_eq!(
                geometry["localToComposition"],
                json!([[0., -1., 36.], [1., 0., 12.]])
            );
        }
        assert_eq!(pixel(&picture, video_point) != RED, active, "frame {frame}");
        assert_eq!(pixel(&picture, cropped_point), RED);
        let hit = pick(&mut session, 0, frame, video_point)?;
        assert_eq!(hit["indeterminate"], false);
        assert_eq!(
            hit["hits"],
            if active {
                json!([video, red])
            } else {
                json!([red])
            },
            "frame {frame}",
        );
        assert_eq!(
            pick(&mut session, 0, frame, cropped_point)?["hits"],
            json!([red]),
        );
    }

    let exposed = [60, 10];
    let base = pick(&mut session, 0, 3, exposed)?;
    assert_eq!(base["hits"], json!([]));
    assert_eq!(base["indeterminate"], false);
    assert_eq!(base["fallbackPlacement"], "V1:shot@0");
    let selection = session.dispatch(EditorCommand::SelectAt {
        revision: 0,
        frame: 3,
        point: center(exposed),
    })?;
    assert_eq!(
        selection["selection"],
        json!({"kind":"placement","occurrence":"V1:shot@0"})
    );

    let lifted = [11, 12];
    let footage = json!({"occurrence":"V1:lifted@8","layers":[0]});
    let picture = session.render(0, 8)?;
    assert_ne!(pixel(&picture, lifted), pixel(&picture, exposed));
    assert_eq!(pick(&mut session, 0, 8, lifted)?["hits"], json!([footage]));
    let miss = pick(&mut session, 0, 8, exposed)?;
    assert_eq!(miss["hits"], json!([]));
    assert_eq!(miss["indeterminate"], false);
    assert!(miss.get("fallbackPlacement").is_none());
    Ok(())
}

#[test]
fn isolated_ancestor_masks_clip_children_in_each_placement() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let sequences = fixture.path().join("sequences");
    fs::create_dir(&sequences)?;
    fs::write(
        fixture.path().join("project.proj"),
        "project clipping fps 24/1 size 64x48\n[0,8) V1 shot\n[8,16) V1 shot\n",
    )?;
    fs::write(
        sequences.join("shot.xml"),
        r##"<sequence background="#000000">
          <rect width="64" height="48" fill="#00ff00"/>
          <g position="42 8" rotation="90">
            <mask><rect y="2" width="18" height="20">
              <animate attributeName="x">
                <key frame="0" value="2"/><key frame="4" value="10"/>
              </animate>
            </rect></mask>
            <g position="2 2">
              <mask><rect x="4" y="4" width="12" height="13"/></mask>
              <rect width="24" height="24" fill="#ff0000"/>
            </g>
          </g>
        </sequence>"##,
    )?;
    let mut session = EditorSession::open(fixture.path())?;
    for (frame, occurrence, first_color) in [
        (1, "V1:shot@0", RED),
        (5, "V1:shot@0", GREEN),
        (9, "V1:shot@8", RED),
    ] {
        let picture = session.render(0, frame)?;
        let front = json!({"occurrence":occurrence,"layers":[1,0,0]});
        let back = json!({"occurrence":occurrence,"layers":[0]});
        let geometry = session.dispatch(EditorCommand::LayerGeometry {
            revision: 0,
            frame,
            layer: LayerRef {
                occurrence: occurrence.into(),
                layers: vec![1, 0, 0],
            },
        })?;
        let mask_x = if frame == 5 { 10. } else { 4. };
        assert_eq!(
            geometry["clips"],
            json!([
                {
                    "localToComposition": [[0.0, -1.0, 42.0], [1.0, 0.0, 8.0]],
                    "maskBounds": [mask_x - 1., 1.0, mask_x + 19., 23.0],
                    "empty": false,
                },
                {
                    "localToComposition": [[0.0, -1.0, 40.0], [1.0, 0.0, 10.0]],
                    "maskBounds": [3.0, 3.0, 17.0, 18.0],
                    "empty": false,
                },
            ])
        );
        for (point, color) in [
            ([33, 16], first_color),
            ([33, 20], RED),
            ([38, 20], GREEN),
            ([12, 20], GREEN),
        ] {
            assert_eq!(
                pixel(&picture, point),
                color,
                "frame {frame}, point {point:?}"
            );
            let expected = if color == RED {
                json!([front, back])
            } else {
                json!([back])
            };
            let hit = pick(&mut session, 0, frame, point)?;
            assert_eq!(
                hit["indeterminate"], false,
                "frame {frame}, point {point:?}"
            );
            assert_eq!(hit["hits"], expected, "frame {frame}, point {point:?}");
        }
    }

    for (point, layers) in [([34., 22.], json!([1, 0, 0])), ([39., 22.], json!([0]))] {
        session.dispatch(EditorCommand::BeginMarquee {
            revision: 0,
            frame: 9,
            point: [point[0] - 2., point[1] - 2.],
            threshold: Some(0.),
        })?;
        let selection = session.dispatch(EditorCommand::FinishMarquee { revision: 0, point })?;
        assert_eq!(selection["selection"]["occurrence"], "V1:shot@8");
        assert_eq!(selection["selection"]["layers"], layers);
    }
    Ok(())
}

#[test]
fn type_on_uses_occupied_lines_and_text_anchors_follow_an_accepted_edit() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let sequences = fixture.path().join("sequences");
    fs::create_dir(&sequences)?;
    fs::write(
        fixture.path().join("project.proj"),
        "project labels fps 24/1 size 180x132\n[0,8) V1 shot\n",
    )?;
    fs::copy(
        example().join("assets/N4GridSans.ttf"),
        fixture.path().join("fixture.ttf"),
    )?;
    let shot = sequences.join("shot.xml");
    let source = r##"<sequence background="#000000">
      <rect width="180" height="132" fill="#00ff00"/>
      <text position="12 8" font="fixture.ttf" width="150" size="24" fill="#000000">
        <copy>W W&#10;WW</copy>
        <type-on><key frame="0" value="1"/><key frame="6" value="6"/></type-on>
      </text>
      <text name="Anchor" position="12 76" font="fixture.ttf" width="150" size="24" fill="#000000">
        <copy>I</copy>
      </text>
      <transform-group position="4 -8">
        <text-anchor target="Anchor" point="last-line-right-baseline"/>
        <rect width="8" height="8" fill="#ffff00"/>
      </transform-group>
    </sequence>"##;
    fs::write(&shot, source)?;
    let mut session = EditorSession::open(fixture.path())?;
    let early = session.render(0, 0)?;
    let full = session.render(0, 6)?;
    let second_line = (8..75)
        .rev()
        .flat_map(|y| (12..162).map(move |x| [x, y]))
        .find(|&point| pixel(&full, point)[1] < 35 && pixel(&early, point) == GREEN)
        .context("revealed line has no dark ink below the prefix")?;
    assert!(second_line[1] > 33, "expected a second line");
    let label = json!({"occurrence":"V1:shot@0","layers":[1]});
    let back = json!({"occurrence":"V1:shot@0","layers":[0]});
    assert_eq!(
        pick(&mut session, 0, 0, second_line)?["hits"],
        json!([back])
    );
    assert_eq!(
        pick(&mut session, 0, 6, second_line)?["hits"],
        json!([label, back])
    );
    // Choosing the line includes the space between its two visible words.
    let ink_columns: Vec<u32> = (12..100)
        .filter(|&x| (8..33).any(|y| pixel(&full, [x, y])[1] < 35))
        .collect();
    let gap = ink_columns
        .windows(2)
        .max_by_key(|x| x[1] - x[0])
        .context("expected two separated words")?;
    assert!(gap[1] - gap[0] > 3);
    let inter_word_space = [(gap[0] + gap[1]) / 2, 20];
    assert_eq!(pixel(&full, inter_word_space), GREEN);
    assert_eq!(
        pick(&mut session, 0, 6, inter_word_space)?["hits"],
        json!([label, back])
    );
    assert_eq!(
        pick(&mut session, 0, 6, [145, second_line[1]])?["hits"],
        json!([back])
    );

    let marker = json!({"occurrence":"V1:shot@0","layers":[3,0]});
    let marker_pixel = |picture: &Frame| {
        (80..122)
            .flat_map(|y| (12..162).map(move |x| [x, y]))
            .find(|&point| pixel(picture, point) == YELLOW)
            .context("the picture has no anchored marker")
    };
    let old = marker_pixel(&full)?;
    assert_eq!(pick(&mut session, 0, 6, old)?["hits"][0], marker);
    fs::write(&shot, source.replace("<copy>I</copy>", "<copy>WWWW</copy>"))?;
    session.dispatch(EditorCommand::Reload {
        revision: 0,
        discard_changes: false,
    })?;
    let updated = session.render(1, 6)?;
    let new = marker_pixel(&updated)?;
    assert!(new[0] > old[0] + 20);
    assert_eq!(pick(&mut session, 1, 6, new)?["hits"][0], marker);
    assert_ne!(pick(&mut session, 1, 6, old)?["hits"][0], marker);
    let stale = pick(&mut session, 0, 6, old).unwrap_err();
    assert_eq!(stale.code, "staleRevision");
    Ok(())
}
