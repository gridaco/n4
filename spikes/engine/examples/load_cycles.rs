//! Pipe-driven probe for fixtures/playback/cycles.py. One preview and one
//! filmstrip stay alive until EOF so allocator and decoder state spans cycles.
use anyhow::{Context, Result, ensure};
use n4_engine::Project;
use n4_engine::preview::{Filmstrip, Preview};
use serde_json::json;
use std::io::{self, BufRead, Write};
use std::path::Path;

fn main() -> Result<()> {
    let project_path = std::env::args_os()
        .nth(1)
        .context("expected project path")?;
    let project = Project::load(Path::new(&project_path))?;
    ensure!(
        project.shots.len() == 1 && project.shots[0].source_in == 0 && project.duration()? == 3600,
        "use the generated 120-second playback load"
    );
    let source = project.shots[0].source.clone();
    let preview = Preview::open(project.clone())?;
    let filmstrip = Filmstrip::open(project, 160)?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{}", json!({"ready": true}))?;
    stdout.flush()?;
    for (i, line) in io::stdin().lock().lines().enumerate() {
        ensure!(line? == "cycle", "expected cycle command");
        // 48 distinct 1080p surfaces exceed the 256 MiB decode LRU window.
        // Every cycle performs the same work for a comparable idle endpoint.
        let mut thumb_ticket = 0;
        for frame in 450..498 {
            thumb_ticket = filmstrip.request((frame * 19) % 3600);
            let visible = preview.wait(preview.request(frame))?;
            ensure!(
                visible.trace.request.timeline_frame == frame
                    && visible.trace.request.source_frame == Some(frame),
                "wrong playback frame"
            );
        }
        let thumb = filmstrip.wait(thumb_ticket)?;
        ensure!(
            thumb.source_frame == (497 * 19) % 3600 && thumb.source == source,
            "wrong filmstrip source"
        );
        let stale = preview.request(3142);
        let stale_thumb = filmstrip.request(3142);
        for n in 0..32 {
            let frame = (n * 179) % 3600;
            preview.request(frame);
            filmstrip.request(frame);
        }
        let final_ticket = preview.request(1300);
        let final_thumb = filmstrip.request(2700);
        ensure!(
            preview.wait(stale).is_err() && filmstrip.wait(stale_thumb).is_err(),
            "superseded request returned a frame"
        );
        let frame = preview.wait(final_ticket)?;
        let thumb = filmstrip.wait(final_thumb)?;
        ensure!(
            frame.trace.request.timeline_frame == 1300
                && frame.trace.request.source_frame == Some(1300),
            "wrong final preview"
        );
        ensure!(
            thumb.source_frame == 2700 && thumb.source == source && thumb.size == [160, 90],
            "wrong final thumbnail"
        );
        drop((frame, thumb));
        writeln!(
            stdout,
            "{}",
            json!({"cycle": i+1, "sequential_frames": 48,
            "preview_ticket": final_ticket, "thumbnail_ticket": final_thumb,
            "superseded_requests_rejected": 2, "last_preview_frame": 1300, "last_thumbnail_frame": 2700})
        )?;
        stdout.flush()?;
    }
    Ok(())
}
