//! One granted project per process. stdin/stdout carry framed control (one JSON
//! object per line); inherited descriptor 3 carries tightly packed RGBA8.
use anyhow::{Context, Result, anyhow, bail, ensure};
use n4_engine::{
    Frame, RevisionChange,
    editor::{EditorCommand, EditorError, EditorSession},
    host::{ThumbnailTicket, Viewer},
    project_watch::ProjectWatch,
    transport::ExportJob,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

#[derive(Deserialize)]
struct Request {
    id: u64,
    #[serde(flatten)]
    command: Command,
    /// Requests queued against an earlier revision can be rejected before use.
    revision: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Command {
    Inspect,
    Stats,
    Reopen {
        frame: Option<u32>,
    },
    Seek {
        frame: u32,
        generation: u64,
        #[serde(default, rename = "previewOnly")]
        preview_only: bool,
    },
    Editor {
        command: EditorCommand,
    },
    Hud {
        frame: u32,
    },
    Thumbnail {
        frame: u32,
        generation: u64,
        placement: Option<String>,
    },
    Play {
        frame: u32,
        generation: u64,
    },
    Next {
        generation: u64,
    },
    Presented {
        frame: u32,
        generation: u64,
    },
    Pause {
        generation: u64,
    },
    Export {
        path: PathBuf,
    },
    ExportStatus,
    CancelExport,
    Close,
}

fn write_control(fields: &Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, fields)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn reply(id: u64, mut fields: Value) -> Result<()> {
    fields["id"] = json!(id);
    if fields.get("ok").is_none() {
        fields["ok"] = json!(true);
    }
    write_control(&fields)
}

fn reject(id: u64, err: anyhow::Error) -> Result<()> {
    if let Some(error) = err.downcast_ref::<EditorError>() {
        return reply(
            id,
            json!({"ok":false, "error":error.message, "editorError":error}),
        );
    }
    reply(id, json!({"ok":false, "error":format!("{err:#}")}))
}

#[derive(Clone, Copy)]
struct Target {
    id: u64,
    frame: u32,
    generation: u64,
}

struct Service {
    export: Option<ExportJob>,
    export_outcome: Value,
    session: Session,
    reopen_after_export: bool,
    generation: u64,
    seek: Option<Target>,
    thumbnail: Option<(Target, ThumbnailTicket)>,
    /// One playback picture awaits renderer submission before another can leave.
    awaiting: Option<u32>,
    pixels: SyncSender<Arc<Vec<u8>>>,
}

enum Session {
    ReadOnly {
        session_id: String,
        project: PathBuf,
        viewer: Viewer,
        watch: ProjectWatch,
    },
    Editable(EditorSession),
}

impl Session {
    fn open(project: PathBuf, editable: bool) -> Result<Self> {
        if editable {
            return Ok(Self::Editable(EditorSession::open(&project)?));
        }
        let viewer = Viewer::open(&project, 0)?;
        let watch = ProjectWatch::new(&viewer.snapshot.inputs)?;
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).context("identify native session")?;
        Ok(Self::ReadOnly {
            session_id: format!("{:032x}", u128::from_be_bytes(id)),
            project,
            viewer,
            watch,
        })
    }

    fn session_id(&self) -> &str {
        match self {
            Self::ReadOnly { session_id, .. } => session_id,
            Self::Editable(editor) => editor.session_id(),
        }
    }

    fn viewer(&self) -> &Viewer {
        match self {
            Self::ReadOnly { viewer, .. } => viewer,
            Self::Editable(editor) => editor.viewer(),
        }
    }

    fn viewer_mut(&mut self) -> &mut Viewer {
        match self {
            Self::ReadOnly { viewer, .. } => viewer,
            Self::Editable(editor) => editor.viewer_mut(),
        }
    }

    fn take_input_change(&self) -> bool {
        match self {
            Self::ReadOnly { watch, .. } => watch.take_changed(),
            Self::Editable(editor) => editor.take_input_change(),
        }
    }

    fn set_playhead(&mut self, frame: u32) -> Result<()> {
        if let Self::Editable(editor) = self {
            editor.set_playhead(frame)?;
        }
        Ok(())
    }

    fn reopen(&mut self, frame: Option<u32>) -> Result<(Frame, RevisionChange)> {
        let Self::ReadOnly {
            viewer,
            watch,
            project,
            ..
        } = self
        else {
            bail!("editable sessions use the editor reload command");
        };
        let reopened = viewer
            .reopen(project, frame)
            .inspect_err(|error| watch.rejected(error))?;
        watch.accepted(&viewer.snapshot.inputs);
        Ok(reopened)
    }
}

impl Service {
    fn refresh_export(&mut self) {
        let Some(job) = &self.export else {
            return;
        };
        self.export_outcome = match job.poll() {
            Ok(Some(report)) => json!({"state":"completed","report":report}),
            Err(_) if job.cancellation_requested() => json!({"state":"cancelled"}),
            Err(err) => json!({"state":"failed","error":format!("{err:#}")}),
            Ok(None) => return,
        };
        self.export = None;
    }

    fn export_status(&self, id: u64) -> Result<()> {
        let status = self.export.as_ref().map_or_else(
            || self.export_outcome.clone(),
            |job| {
                let state = if job.cancellation_requested() { "cancelling" } else { "running" };
                json!({"state":state,"rendered":job.rendered(),"frames":self.session.viewer().inspection.duration})
            },
        );
        reply(id, json!({"status":status}))
    }

    fn include_inspection(&self, fields: &mut Value) {
        fields["protocol"] = json!(4);
        fields["sessionId"] = json!(self.session.session_id());
        fields["revision"] = json!(self.session.viewer().snapshot.revision);
        fields["snapshot"] = json!(self.session.viewer().inspection);
        fields["peaks"] = json!(self.session.viewer().prepared.peaks.as_ref());
        fields["size"] = json!(self.session.viewer().snapshot.document.project.size);
        if matches!(self.session, Session::Editable(_)) {
            fields["editable"] = json!(true);
        }
    }

    fn advance(&mut self, generation: u64) -> Result<()> {
        ensure!(generation >= self.generation, "stale generation");
        self.generation = generation;
        self.awaiting = None;
        if let Some(prior) = self.seek.take() {
            reject(prior.id, anyhow!("seek superseded"))?;
        }
        Ok(())
    }

    fn queue_pixels(&self, target: Target, size: [u32; 2], rgba: Arc<Vec<u8>>) -> Result<Value> {
        let bytes = rgba.len();
        // Queue the bytes in the same order as their metadata. The reader drains
        // the binary channel independently. Capacity is bounded at both ends.
        self.pixels.send(rgba).context("binary channel closed")?;
        Ok(
            json!({"revision":self.session.viewer().snapshot.revision, "frame":target.frame,
            "generation":target.generation, "width":size[0],"height":size[1],"bytes":bytes}),
        )
    }

    fn send_pixels(&self, target: Target, size: [u32; 2], rgba: Arc<Vec<u8>>) -> Result<()> {
        reply(target.id, self.queue_pixels(target, size, rgba)?)
    }

    fn send_frame(&self, target: Target, picture: Frame) -> Result<()> {
        ensure!(
            target.frame == picture.trace.request.timeline_frame,
            "wrong native frame"
        );
        ensure!(
            picture.trace.revision == self.session.viewer().snapshot.revision,
            "wrong native revision"
        );
        self.send_pixels(target, picture.size, picture.rgba)
    }

    fn finish_transition(
        &mut self,
        id: u64,
        picture: Frame,
        change: RevisionChange,
    ) -> Result<Value> {
        self.advance(self.generation)?;
        if let Some((prior, _)) = self
            .thumbnail
            .take_if(|(target, ticket)| !ticket.survives(&change, target.frame))
        {
            reject(prior.id, anyhow!("thumbnail superseded by reopen"))?;
        }
        let target = Target {
            id,
            frame: picture.trace.request.timeline_frame,
            generation: self.generation,
        };
        let mut metadata = self.queue_pixels(target, picture.size, picture.rgba)?;
        metadata["change"] = json!(change);
        self.include_inspection(&mut metadata);
        Ok(metadata)
    }

    fn editor_command(&mut self, id: u64, command: EditorCommand) -> Result<()> {
        ensure!(
            !matches!(command, EditorCommand::Render { .. }),
            "use the viewer seek command for pictures"
        );
        if matches!(
            command,
            EditorCommand::SetTrackEnabled { .. }
                | EditorCommand::Undo { .. }
                | EditorCommand::Redo { .. }
                | EditorCommand::Reload { .. }
        ) {
            if self.export.is_some() && matches!(command, EditorCommand::Reload { .. }) {
                self.reopen_after_export = true;
            }
            ensure!(self.export.is_none(), "export in progress");
        }
        let Session::Editable(editor) = &mut self.session else {
            bail!("the native session is read-only");
        };
        let response = editor.dispatch_with_picture(command)?;
        let mut metadata = if let Some((picture, change)) = response.transition {
            self.reopen_after_export = false;
            self.finish_transition(id, picture, change)?
        } else {
            json!({})
        };
        metadata["result"] = response.result;
        reply(id, metadata)
    }

    fn hud(&self, id: u64, frame: u32) -> Result<()> {
        let Session::Editable(editor) = &self.session else {
            bail!("the native session is read-only");
        };
        let hud_version = editor.hud_version();
        let Some(image) = editor.render_hud(frame)? else {
            return reply(id, json!({"empty":true,"hudVersion":hud_version}));
        };
        let target = Target {
            id,
            frame,
            generation: self.generation,
        };
        let mut fields = self.queue_pixels(target, image.size, image.rgba)?;
        fields["x"] = json!(image.origin[0]);
        fields["y"] = json!(image.origin[1]);
        fields["hudVersion"] = json!(hud_version);
        reply(id, fields)
    }

    fn command(&mut self, req: Request) -> Result<bool> {
        self.refresh_export();
        let id = req.id;
        if let Some(expected) = req.revision {
            ensure!(
                expected == self.session.viewer().snapshot.revision,
                "stale revision"
            );
        }
        if self.export.is_some() && matches!(&req.command, Command::Reopen { .. }) {
            // Export may have started after we sent a change event. Deliver
            // another hint when it finishes so clients need no retry timer.
            self.reopen_after_export = true;
        }
        if matches!(
            &req.command,
            Command::Seek { .. }
                | Command::Play { .. }
                | Command::Reopen { .. }
                | Command::Export { .. }
        ) {
            ensure!(self.export.is_none(), "export in progress");
        }
        match req.command {
            Command::Inspect => {
                let mut fields = json!({});
                self.include_inspection(&mut fields);
                reply(id, fields)?;
            }
            Command::Stats => {
                reply(
                    id,
                    json!({
                        "revision": self.session.viewer().snapshot.revision,
                        "audioPreparations": self.session.viewer().audio_preparations(),
                        "frames": self.session.viewer().frame_cache_stats()
                    }),
                )?;
            }
            Command::Close => {
                reply(id, json!({}))?;
                return Ok(false);
            }
            Command::Reopen { frame } => {
                self.reopen_after_export = false;
                let (picture, change) = self.session.reopen(frame)?;
                reply(id, self.finish_transition(id, picture, change)?)?;
            }
            Command::Editor { command } => self.editor_command(id, command)?,
            Command::Hud { frame } => self.hud(id, frame)?,
            Command::Seek {
                frame,
                generation,
                preview_only,
            } => {
                self.session.viewer().check_frame(frame)?;
                self.advance(generation)?;
                self.session.viewer_mut().seek(frame)?;
                if !preview_only {
                    self.session.set_playhead(frame)?;
                }
                self.seek = Some(Target {
                    id,
                    frame,
                    generation,
                });
            }
            Command::Thumbnail {
                frame,
                generation,
                placement,
            } => {
                let ticket = self
                    .session
                    .viewer_mut()
                    .request_thumbnail(frame, placement.as_deref())?;
                let request = Target {
                    id,
                    frame,
                    generation,
                };
                if let Some((prior, _)) = self.thumbnail.replace((request, ticket)) {
                    reject(prior.id, anyhow!("thumbnail superseded"))?;
                }
            }
            Command::Play { frame, generation } => {
                self.session.viewer().check_frame(frame)?;
                self.advance(generation)?;
                self.session.viewer_mut().play(frame)?;
                self.session.set_playhead(frame)?;
                reply(
                    id,
                    json!({"revision":self.session.viewer().snapshot.revision,"generation":generation,"playing":true}),
                )?;
            }
            Command::Next { generation } => {
                ensure!(generation == self.generation, "stale generation");
                if self.session.viewer().controller.wants_play()
                    && self.awaiting.is_none()
                    && let Some(picture) = self.session.viewer_mut().poll()?
                {
                    let frame = picture.trace.request.timeline_frame;
                    self.awaiting = Some(frame);
                    self.send_frame(
                        Target {
                            id,
                            frame,
                            generation,
                        },
                        picture,
                    )?;
                } else {
                    reply(
                        id,
                        json!({"revision":self.session.viewer().snapshot.revision,"generation":generation,
                        "playing":self.session.viewer().controller.wants_play(),"bytes":0}),
                    )?;
                }
            }
            Command::Presented { frame, generation } => {
                ensure!(
                    generation == self.generation && self.awaiting == Some(frame),
                    "stale or unsolicited presentation acknowledgement"
                );
                self.awaiting = None;
                self.session.viewer_mut().controller.receiving(frame);
                self.session.viewer_mut().controller.after_present(true)?;
                self.session.set_playhead(frame)?;
                reply(id, json!({}))?;
            }
            Command::Pause { generation } => {
                self.advance(generation)?;
                self.session.viewer_mut().controller.pause();
                reply(
                    id,
                    json!({"revision":self.session.viewer().snapshot.revision,"generation":generation,"playing":false}),
                )?;
            }
            Command::Export { path } => {
                self.advance(self.generation)?;
                self.export = Some(self.session.viewer_mut().export_mp4(path)?);
                self.export_status(id)?;
            }
            Command::ExportStatus => self.export_status(id)?,
            Command::CancelExport => {
                if let Some(job) = &self.export {
                    job.cancel();
                }
                self.export_status(id)?;
            }
        }
        Ok(true)
    }

    fn poll(&mut self) -> Result<()> {
        if let Some(target) = self.seek
            && let Some(result) = self.session.viewer_mut().poll().transpose()
        {
            self.seek = None;
            match result {
                Ok(picture) => {
                    self.session.viewer_mut().controller.receiving(target.frame);
                    self.send_frame(target, picture)?;
                }
                Err(err) => reject(target.id, err)?,
            }
        }
        if let Some((target, ticket)) = &self.thumbnail
            && let Some(result) = ticket.try_take().transpose()
        {
            let target = *target;
            let revision = ticket.revision;
            self.thumbnail = None;
            match result {
                Ok(thumb) => {
                    let mut fields = self.queue_pixels(target, thumb.size, thumb.rgba)?;
                    fields["revision"] = json!(revision);
                    reply(target.id, fields)?;
                }
                Err(err) => reject(target.id, err)?,
            }
        }
        Ok(())
    }

    fn run(&mut self, commands: Receiver<io::Result<String>>) -> Result<()> {
        loop {
            self.refresh_export();
            if self.export.is_none()
                && (self.session.take_input_change() || self.reopen_after_export)
            {
                self.reopen_after_export = false;
                write_control(
                    &json!({"event":"inputsChanged","revision":self.session.viewer().snapshot.revision}),
                )?;
            }
            let timeout = if self.seek.is_some() || self.thumbnail.is_some() {
                self.poll()?;
                Duration::from_millis(1)
            } else {
                // The engine observes files on its own worker. Only check its
                // coalesced signal while waiting for the next host command.
                Duration::from_millis(100)
            };
            let line = match commands.recv_timeout(timeout) {
                Ok(line) => line?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    let id = req.id;
                    match self.command(req) {
                        Ok(false) => break,
                        Err(err) => reject(id, err)?,
                        Ok(true) => {}
                    }
                }
                Err(err) => {
                    let id = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| v["id"].as_u64())
                        .unwrap_or(0);
                    reject(id, err.into())?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn frame_pipe() -> Result<File> {
    use std::os::fd::FromRawFd;
    // Set close-on-exec before loading any media. Otherwise an FFmpeg child
    // could keep the socket alive after the host disconnects.
    let fd = 3;
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    ensure!(
        flags >= 0,
        "n4-host expects an inherited binary pipe on descriptor 3"
    );
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    // The dedicated descriptor is transferred to this process by the launcher.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn frame_pipe() -> Result<File> {
    bail!("the embedded host currently requires Unix file descriptors")
}

fn main() -> Result<()> {
    let mut binary = frame_pipe()?;
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: n4-host [--editor] <project>";
    let first = args.next().context(usage)?;
    let editable = first == "--editor";
    let project = if editable {
        args.next().context(usage)?
    } else {
        first
    };
    if args.next().is_some() {
        bail!("{usage}");
    }
    let session = Session::open(project.into(), editable)?;
    let (pixels, packets) = mpsc::sync_channel::<Arc<Vec<u8>>>(1);
    thread::Builder::new()
        .name("n4-host-frames".into())
        .spawn(move || {
            for bytes in packets {
                if binary.write_all(&bytes).is_err() {
                    break;
                }
            }
        })?;
    let (input, requests) = mpsc::sync_channel(8);
    thread::Builder::new()
        .name("n4-host-input".into())
        .spawn(move || {
            for line in io::stdin().lock().lines() {
                if input.send(line).is_err() {
                    break;
                }
            }
        })?;
    Service {
        export: None,
        export_outcome: json!({"state":"idle"}),
        session,
        reopen_after_export: false,
        generation: 0,
        seek: None,
        thumbnail: None,
        awaiting: None,
        pixels,
    }
    .run(requests)
}
