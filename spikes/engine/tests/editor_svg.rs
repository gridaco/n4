#![cfg(feature = "host-audio")]

use anyhow::{Context, Result};
use n4_engine::{
    Frame,
    editor::{EditorCommand, EditorSession, LayerRef, Visibility},
};
use serde_json::json;
use std::{fs, path::Path};

fn pixel(picture: &Frame, [x, y]: [u32; 2]) -> [u8; 4] {
    let offset = ((y * picture.size[0] + x) * 4) as usize;
    picture.rgba[offset..offset + 4].try_into().unwrap()
}

#[test]
fn station_symbol_is_one_pickable_layer_with_a_native_outline() -> Result<()> {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/signal-atlas");
    let mut session = EditorSession::open(&project)?;
    let frame = 42;
    let picture = session.render(0, frame)?;
    let ink = (754..770)
        .map(|x| [x, 276])
        .find(|&point| pixel(&picture, point) == [243, 239, 223, 255])
        .context("logo has no opaque ink on the witness row")?;

    let logo = LayerRef {
        occurrence: "V1:survey@0".into(),
        layers: vec![0, 2],
    };
    let pick = session.dispatch(EditorCommand::HitTest {
        revision: 0,
        frame,
        point: ink.map(|coordinate| coordinate as f64 + 0.5),
    })?;
    assert_eq!(pick["hits"][0], json!(logo));
    assert_eq!(pick["indeterminate"], false);
    for point in [[762, 276], [10, 10]] {
        assert_ne!(pixel(&picture, point), [243, 239, 223, 255]);
        let pick = session.dispatch(EditorCommand::HitTest {
            revision: 0,
            frame,
            point: point.map(|coordinate| coordinate as f64 + 0.5),
        })?;
        assert!(!pick["hits"].as_array().unwrap().contains(&json!(logo)));
        assert_eq!(pick["indeterminate"], false);
    }

    session.dispatch(EditorCommand::Seek { frame })?;
    session.dispatch(EditorCommand::SelectAt {
        revision: 0,
        frame,
        point: ink.map(|coordinate| coordinate as f64 + 0.5),
    })?;
    let geometry = session.state().hud.context("logo has no HUD geometry")?;
    assert_eq!(geometry.target, logo);
    assert_eq!(geometry.visibility, Visibility::Visible);
    assert_eq!(geometry.local_bounds, Some([0., 0., 16., 16.]));
    let tile = session.render_hud(frame)?.context("logo has no outline")?;
    assert!(tile.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    Ok(())
}

#[test]
fn svg_holes_pick_through_and_rejected_assets_keep_the_accepted_pixels() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let sequences = fixture.path().join("sequences");
    fs::create_dir(&sequences)?;
    fs::write(
        fixture.path().join("project.proj"),
        "project artwork fps 24/1 size 64x36\n[0,8) V1 plate\n[0,8) V2 ring\n",
    )?;
    fs::write(
        sequences.join("plate.xml"),
        r##"<sequence background="#000000"><rect x="8" y="8" width="24" height="20" fill="#00ff00"/></sequence>"##,
    )?;
    fs::write(
        sequences.join("ring.xml"),
        r#"<sequence><svg name="Ring" position="8 8" source="ring.svg"/></sequence>"#,
    )?;
    let svg = fixture.path().join("ring.svg");
    fs::write(
        &svg,
        r##"<svg width="24" height="20"><path fill="#ff0000" d="M0 0 H24 V20 H0 Z M8 6 V14 H16 V6 Z"/></svg>"##,
    )?;
    let mut session = EditorSession::open(fixture.path())?;
    let first = session.render(0, 1)?;
    assert_eq!(pixel(&first, [10, 10]), [255, 0, 0, 255]);
    assert_eq!(pixel(&first, [20, 18]), [0, 255, 0, 255]);
    let test_hover = |session: &mut EditorSession, revision, frame, point| -> Result<_> {
        Ok(session.dispatch(EditorCommand::HoverAt {
            revision,
            frame,
            point,
        })?["hover"]
            .clone())
    };
    let overlay = json!({"occurrence":"V2:ring@0","layers":[0]});
    let plate = json!({"occurrence":"V1:plate@0","layers":[0]});
    assert_eq!(test_hover(&mut session, 0, 1, [10.5, 10.5])?, overlay);
    assert_eq!(test_hover(&mut session, 0, 1, [20.5, 18.5])?, plate);
    assert_eq!(test_hover(&mut session, 0, 1, [40.5, 32.5])?, json!(null));

    fs::write(&svg, "<invalid")?;
    let rejected = session.dispatch(EditorCommand::Reload {
        revision: 0,
        discard_changes: false,
    });
    assert!(rejected.is_err());
    assert_eq!(session.state().revision, 0);
    for _ in 0..20 {
        assert_eq!(test_hover(&mut session, 0, 2, [10.5, 10.5])?, overlay);
        assert_eq!(test_hover(&mut session, 0, 2, [20.5, 18.5])?, plate);
    }
    assert_eq!(session.render(0, 2)?.rgba, first.rgba);

    fs::write(
        &svg,
        r##"<svg width="24" height="20"><path fill="#0000ff" d="M0 0 H24 V20 H0 Z"/></svg>"##,
    )?;
    session.dispatch(EditorCommand::Reload {
        revision: 0,
        discard_changes: false,
    })?;
    assert_eq!(session.state().revision, 1);
    assert_eq!(test_hover(&mut session, 1, 3, [20.5, 18.5])?, overlay);
    assert_eq!(pixel(&session.render(1, 3)?, [20, 18]), [0, 0, 255, 255]);
    Ok(())
}
