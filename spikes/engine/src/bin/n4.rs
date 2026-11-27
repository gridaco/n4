use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use n4_engine::{AuthoringDocument, Engine, Project};
use std::{
    fs,
    io::{BufRead, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(about = "N4 headless native frame study")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the accepted hierarchy, shared definitions and their authored keyframes.
    Inspect {
        project: PathBuf,
        /// Limit output to one occurrence path from the full inspection document.
        #[arg(long)]
        placement: Option<String>,
        /// Follow the named layer path inside this occurrence, one --layer per level.
        #[arg(long = "layer", requires = "property")]
        layers: Vec<String>,
        /// Inspect a property and its keys on the local and project clocks.
        #[arg(long, requires_all = ["placement", "layers"])]
        property: Option<String>,
        /// Sample the selected property at this project frame.
        #[arg(long, requires = "property")]
        frame: Option<u32>,
    },
    /// Check an authored document and its assets. Supply a pending detail in its authored format on stdin.
    Validate {
        project: PathBuf,
        #[arg(long)]
        detail: Option<String>,
    },
    /// Decode the authored detail into structured values before resolving paths and timing.
    ReadDetail {
        project: PathBuf,
        #[arg(long)]
        detail: String,
        #[arg(long)]
        stdin: bool,
    },
    /// Measure a named label in its local space with the same paragraph layout as the painter.
    TextMetrics {
        project: PathBuf,
        #[arg(long)]
        detail: String,
        #[arg(long = "layer", required = true)]
        layers: Vec<String>,
        /// Read the unsaved detail on stdin instead of its file.
        #[arg(long)]
        stdin: bool,
    },
    /// Inspect a media source or composed shot thumbnail at a project frame.
    Thumbnail {
        project: PathBuf,
        #[arg(long)]
        frame: u32,
        #[arg(long, default_value_t = 160)]
        width: u32,
        #[arg(long)]
        out: PathBuf,
    },
    /// Export the pinned edit to a new H.264/AAC MP4.
    Export {
        project: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Apply newline-delimited edit commands, replying once per command.
    Session { project: PathBuf },
    /// Open an editor session with newline-delimited JSON requests and replies.
    #[cfg(feature = "host-audio")]
    Editor { project: PathBuf },
    /// Render exactly the requested frames, in order, through one engine instance.
    Render {
        project: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        frames: Vec<u32>,
        /// A new directory for the PNG frames and JSON receipt.
        #[arg(long)]
        out: PathBuf,
        /// Change the frame at which the title entrance starts.
        #[arg(long)]
        title_start: Option<u32>,
        /// Change the authored paragraph width in pixels.
        #[arg(long)]
        title_width: Option<u32>,
        /// Finish path-traced scenes with their authored recipe before writing each frame.
        /// Without it such a scene is written as a rasterized Draft unless its Final is stored.
        #[arg(long = "final")]
        finished: bool,
        /// Keep finished path-traced pictures in this directory and reuse the ones found there.
        #[arg(long)]
        final_store: Option<PathBuf>,
    },
}

fn authoring(project: &Path, pending: Option<&str>) -> Result<AuthoringDocument> {
    let mut bytes = Vec::new();
    if pending.is_some() {
        std::io::stdin().lock().read_to_end(&mut bytes)?;
    }
    AuthoringDocument::load(project, pending.map(|id| (id, bytes.as_slice())))
}

fn save_png(path: &Path, size: [u32; 2], rgba: &[u8]) -> Result<()> {
    let output = BufWriter::new(fs::File::create_new(path)?);
    let mut png = png::Encoder::new(output, size[0], size[1]);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
    let mut writer = png.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(())
}

fn inspect_document(
    project: &Path,
    path: Option<&str>,
    layers: &[String],
    property: Option<&str>,
    frame: Option<u32>,
) -> Result<()> {
    use serde_json::json;
    let model = authoring(project, None)?.inspection()?;
    let mut out = BufWriter::new(std::io::stdout().lock());
    if let Some(path) = path {
        let item = model
            .placement(path)
            .with_context(|| format!("unknown placement {path:?}"))?;
        let definition = item
            .definition
            .as_ref()
            .and_then(|id| model.definitions.get(id));
        let response = if let Some(name) = property {
            let definition =
                definition.context("choose a placement with an authored definition")?;
            let (owner, track) = definition.property(layers, name)?;
            let origin = item.project[0]
                .checked_add(owner.start)
                .context("layer clock exceeds project")?;
            let end = owner.end.map_or(item.project[1], |end| {
                item.project[0].saturating_add(end).min(item.project[1])
            });
            let mut keys = Vec::with_capacity(track.keys.len());
            for key in &track.keys {
                let film = u64::from(origin) + u64::from(key.frame);
                keys.push(json!({
                    "layer_frame": key.frame,
                    "project_frame": film,
                    "value": key.value,
                    "ease": key.ease,
                    "inside_placement": film < u64::from(end),
                }));
            }
            let sample = match frame {
                Some(frame) => {
                    ensure!(
                        (origin..end).contains(&frame),
                        "property owner is inactive at project frame {frame}"
                    );
                    let age = frame - origin;
                    Some(
                        json!({"project_frame": frame, "layer_frame": age, "value": track.sample(age, 0.)}),
                    )
                }
                None => None,
            };
            json!({"placement": path, "file": definition.file, "layers": layers,
                   "property": name, "keys": keys, "sample": sample})
        } else {
            let uses: Vec<_> = item
                .definition
                .iter()
                .flat_map(|id| model.uses(id))
                .map(|p| p.path.as_str())
                .collect();
            json!({"placement": item, "definition": definition, "uses": uses})
        };
        serde_json::to_writer_pretty(&mut out, &response)?;
    } else {
        serde_json::to_writer_pretty(&mut out, &model)?;
    }
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Inspect {
            project,
            placement,
            layers,
            property,
            frame,
        } => inspect_document(
            &project,
            placement.as_deref(),
            &layers,
            property.as_deref(),
            frame,
        ),
        Command::Validate { project, detail } => {
            let validate = || -> Result<_> {
                let doc = authoring(&project, detail.as_deref())?;
                doc.validate_assets()?;
                Ok(
                    serde_json::json!({"details": doc.detail_ids().collect::<Vec<_>>(), "detail_files":doc.detail_files()}),
                )
            };
            let receipt = validate().with_context(|| match detail {
                Some(id) => format!("validate candidate detail {id} in {}", project.display()),
                None => format!("validate {}", project.display()),
            })?;
            println!("{receipt}");
            Ok(())
        }
        Command::ReadDetail {
            project,
            detail,
            stdin,
        } => {
            let doc = authoring(&project, stdin.then_some(detail.as_str()))?;
            println!("{}", serde_json::to_string(doc.detail_value(&detail)?)?);
            Ok(())
        }
        Command::TextMetrics {
            project,
            detail,
            layers,
            stdin,
        } => {
            let doc = authoring(&project, stdin.then_some(detail.as_str()))?;
            let layer = doc.layer(&detail, &layers)?;
            let metrics = n4_engine::graphics::text_metrics(layer)
                .with_context(|| format!("measure detail {detail} / {}", layers.join(" / ")))?;
            println!("{}", serde_json::to_string_pretty(&metrics)?);
            Ok(())
        }
        Command::Thumbnail {
            project,
            frame,
            width,
            out,
        } => {
            let filmstrip = n4_engine::preview::Filmstrip::open(Project::load(&project)?, width)?;
            let image = filmstrip.wait(filmstrip.request(frame))?;
            save_png(&out, image.size, &image.rgba)?;
            let receipt = match image.source {
                Some(source) => {
                    serde_json::json!({"project_frame":frame, "source":source, "source_frame":image.source_frame, "size":image.size})
                }
                None => {
                    serde_json::json!({"project_frame":frame, "shot_frame":image.source_frame, "size":image.size})
                }
            };
            println!("{receipt}");
            Ok(())
        }
        Command::Export { project, out } => {
            let mut engine = Engine::new(Project::load(&project)?)?;
            let report = n4_engine::export::write_mp4(&mut engine, &out)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Command::Session { project } => run_session(&project),
        #[cfg(feature = "host-audio")]
        Command::Editor { project } => run_editor(&project),
        Command::Render {
            project,
            frames,
            out,
            title_start,
            title_width,
            finished,
            final_store,
        } => render_images(
            project,
            frames,
            out,
            title_start,
            title_width,
            finished,
            final_store,
        ),
    }
}

#[cfg(feature = "host-audio")]
fn run_editor(project: &Path) -> Result<()> {
    use n4_engine::editor::EditorSession;
    use serde_json::json;
    use std::{
        sync::mpsc::{RecvTimeoutError, sync_channel},
        thread,
        time::Duration,
    };

    let mut session = EditorSession::open(project)?;
    let (send, requests) = sync_channel(16);
    thread::Builder::new()
        .name("n4-editor-input".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        })?;
    let mut output = BufWriter::new(std::io::stdout().lock());
    loop {
        let line = match requests.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => Some(line?),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        if let Some(event) = session.poll_changes() {
            let event = match event {
                Ok(result) => json!({"event": "stateChanged", "result": result}),
                Err(error) => json!({"event": "stateChanged", "error": error}),
            };
            writeln!(output, "{event}")?;
            output.flush()?;
        }
        if let Some(line) = line {
            let response = editor_reply(&mut session, &line);
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
}

#[cfg(feature = "host-audio")]
fn editor_reply(session: &mut n4_engine::editor::EditorSession, line: &str) -> serde_json::Value {
    use serde_json::{Map, Value, json};

    let invalid_request = |id: Option<u64>, message: &str| {
        let error = json!({"code": "invalidRequest", "message": message});
        json!({"id": id, "error": error})
    };
    let mut fields: Map<String, Value> = match serde_json::from_str(line) {
        Ok(fields) => fields,
        Err(error) => return invalid_request(None, &error.to_string()),
    };
    let Some(id) = fields.remove("id").and_then(|value| value.as_u64()) else {
        return invalid_request(None, "expected a numeric request ID");
    };
    let command = match serde_json::from_value(Value::Object(fields)) {
        Ok(command) => command,
        Err(error) => return invalid_request(Some(id), &error.to_string()),
    };
    match session.dispatch(command) {
        Ok(result) => json!({"id": id, "result": result}),
        Err(error) => json!({"id": id, "error": error}),
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Set {
        at: u64,
        path: String,
        value: serde_json::Value,
    },
    Undo {
        at: u64,
    },
    Redo {
        at: u64,
    },
    Relink {
        at: u64,
        from: PathBuf,
        to: PathBuf,
    },
    Reload {
        at: u64,
    },
    Refresh {
        at: u64,
    },
    Save {
        at: u64,
    },
    Inspect {
        frame: u32,
    },
}

fn run_session(project: &Path) -> Result<()> {
    use serde_json::json;
    let mut session = n4_engine::session::Session::open(project)?;
    let mut output = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let result = (|| -> Result<_> {
            match serde_json::from_str(&line)? {
                Edit::Set { at, path, value } => session.set(at, &path, value)?,
                Edit::Save { at } => session.save(at)?,
                Edit::Undo { at } => session.undo(at)?,
                Edit::Redo { at } => session.redo(at)?,
                Edit::Relink { at, from, to } => session.relink(at, &from, &to)?,
                Edit::Reload { at } => session.reload(at)?,
                Edit::Refresh { at } => session.refresh_assets(at)?,
                Edit::Inspect { frame } => return Ok(json!(session.render(frame)?.trace)),
            }
            Ok(serde_json::Value::Null)
        })();
        let response = match result {
            Ok(value) => json!({"ok": true, "revision": session.revision(), "result": value}),
            Err(e) => {
                json!({"ok": false, "revision": session.revision(), "error": format!("{e:#}")})
            }
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}

fn render_images(
    project: PathBuf,
    frames: Vec<u32>,
    out: PathBuf,
    title_start: Option<u32>,
    title_width: Option<u32>,
    finished: bool,
    final_store: Option<PathBuf>,
) -> Result<()> {
    ensure!(
        !out.exists(),
        "choose a new output directory: {}",
        out.display()
    );
    let mut engine = Engine::new(Project::load(&project)?)?;
    engine.edit_title(title_start, title_width)?;
    if let Some(directory) = final_store {
        engine.persist_finals(directory)?;
    }
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = tempfile::Builder::new()
        .prefix(".n4-render-")
        .tempdir_in(parent)
        .with_context(|| format!("prepare output beside {}", out.display()))?;
    let mut results = Vec::new();
    for (i, number) in frames.into_iter().enumerate() {
        let started = std::time::Instant::now();
        let (f, traced) = if finished {
            engine.render_final(number)
        } else {
            engine.render(number).map(|f| (f, 0))
        }
        .with_context(|| format!("render frame {number}"))?;
        let name = format!("{i:03}-f{number:04}.png");
        save_png(&staging.path().join(&name), f.size, &f.rgba)?;
        let mut result = serde_json::json!({ "file": name, "trace": f.trace });
        if finished {
            result["path_traced_scenes"] = traced.into();
            result["seconds"] = started.elapsed().as_secs_f64().into();
        }
        results.push(result);
    }
    let receipt = serde_json::json!({ "adapter": engine.adapter_name(), "frames": results });
    fs::write(
        staging.path().join("receipt.json"),
        serde_json::to_string_pretty(&receipt)?,
    )?;
    fs::rename(staging.path(), &out).with_context(|| format!("publish {}", out.display()))?;
    println!(
        "Rendered {} frame(s) on {} into {}",
        results.len(),
        engine.adapter_name(),
        out.display()
    );
    Ok(())
}
