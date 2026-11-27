#![cfg(all(feature = "host-audio", unix))]

use anyhow::Result;
use n4_engine::editor::{EditorCommand, EditorSession};
use serde_json::json;
use std::{fs, os::unix::fs::symlink, path::Path};

#[test]
fn public_root_overlay_edit_preserves_nested_artwork_and_round_trips() -> Result<()> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/signal-atlas");
    let temp = tempfile::tempdir()?;
    let master = temp.path().join("project.proj");
    fs::copy(source.join("project.proj"), &master)?;
    for dir in ["assets", "sequences"] {
        symlink(source.join(dir).canonicalize()?, temp.path().join(dir))?;
    }
    let original = fs::read_to_string(&master)?;
    let mut session = EditorSession::open(&master)?;
    session.dispatch(EditorCommand::Seek { frame: 42 })?;
    let opening = session.render(0, 42)?;
    let gap = session.render(0, 86)?;
    let reprise = session.render(0, 140)?;

    let edit = session.dispatch(EditorCommand::SetTrackEnabled {
        revision: 0,
        scope: "root".into(),
        track: "V2".into(),
        enabled: false,
    })?;
    assert_eq!(
        edit["change"]["damage"],
        json!({"picture": {"kind": "ranges", "ranges": [[0,56],[128,192]]}, "audio": {"kind": "none"}})
    );
    let edited = session.render(1, 42)?;
    assert_ne!(edited.rgba, opening.rgba);
    assert_eq!(session.render(1, 86)?.rgba, gap.rgba);
    assert_ne!(session.render(1, 140)?.rgba, reprise.rgba);
    assert_eq!(fs::read_to_string(&master)?, original);

    session.dispatch(EditorCommand::Undo { revision: 1 })?;
    assert_eq!(session.render(2, 42)?.rgba, opening.rgba);
    session.dispatch(EditorCommand::Redo { revision: 2 })?;
    assert_eq!(session.render(3, 42)?.rgba, edited.rgba);
    session.dispatch(EditorCommand::Save { revision: 3 })?;
    let (header, body) = original.split_once('\n').unwrap();
    assert_eq!(
        fs::read_to_string(&master)?,
        format!("{header}\ntrack V2 off\n{body}")
    );

    drop(session);
    let mut reopened = EditorSession::open(&master)?;
    let state = reopened.state();
    let lane = state.tracks.iter().find(|lane| lane.track == "V2").unwrap();
    assert!(!lane.enabled);
    assert!(!state.dirty && !state.can_undo);
    assert_eq!(reopened.render(0, 42)?.rgba, edited.rgba);
    assert_eq!(reopened.render(0, 86)?.rgba, gap.rgba);
    Ok(())
}
