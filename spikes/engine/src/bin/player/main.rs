//! A native interactive timeline on the same engine used by n4 export.
mod egui_canvas;
mod exercise;
mod inspector;
mod inspector_host;
mod inspector_trial;
mod playback_trial;
mod release_trial;
mod reopen;
mod scrub;
mod timeline;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use exercise::{Exercise, Snapshot};
use inspector::{Inspector, InspectorAction};
use inspector_host::InspectorHost;
use n4_engine::host::Controller;
use n4_engine::host::{PreparedSound, ProjectSnapshot, Sound};
use n4_engine::{
    AuthoringDocument, preview::Thumbnail, project_watch::ProjectWatch, transport::Transport,
};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::{Keycode, Mod},
    mouse::MouseButton,
    pixels::PixelFormatEnum,
    render::WindowCanvas,
};
use std::{
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::Instant,
};
use timeline::{Hit, Layout, Timeline};

#[derive(Parser)]
#[command(about = "Scrub and play an N4 project on the native timeline")]
struct Args {
    /// Project directory containing project.proj, or an explicit project file.
    #[arg(default_value=concat!(env!("CARGO_MANIFEST_DIR"),"/fixtures/av-cut/edit.json"))]
    project: PathBuf,
    /// Open the read-only authored timeline and property inspector (I toggles it).
    #[arg(long)]
    inspector: bool,
    /// Open a placement, optionally selecting a layer and animated property.
    #[arg(long, num_args=1..=3,value_names=["PLACEMENT","LAYER","PROPERTY"],requires="inspector")]
    focus: Vec<String>,
    /// Capture an inspection after exercising named controls through the SDL event loop.
    #[arg(long, hide=true, requires_all=["inspector", "report"], conflicts_with_all=["scrub_trial","exercise","release_trial","playback_trial"])]
    inspect_trial: bool,
    /// After settling the inspector clicks, inject F5 when this coordination file exists.
    #[arg(long, hide = true, requires = "inspect_trial")]
    inspect_reopen_after: Option<PathBuf>,
    /// Capture the next accepted disk edit without injecting a reload key.
    #[arg(
        long,
        hide = true,
        requires = "inspect_trial",
        conflicts_with = "inspect_reopen_after"
    )]
    inspect_live_reload: bool,
    #[arg(long, hide = true, requires = "inspect_trial")]
    inspect_click: Vec<String>,
    /// Drive the ordinary SDL input path through a finite interaction regression.
    #[arg(long, hide = true)]
    exercise: bool,
    /// Replay a timed drag and record presented playhead and movie frames.
    #[arg(long, conflicts_with = "exercise", requires = "report")]
    scrub_trial: bool,
    /// Restrict the trial's pointer gesture to this half-open interval of film frames.
    #[arg(long, num_args = 2, value_names = ["START", "END"], requires = "scrub_trial")]
    scrub_range: Option<Vec<u32>>,
    /// Independent pointer replay frequency (defaults to 120 Hz).
    #[arg(long, requires = "scrub_trial", value_parser = clap::value_parser!(u32).range(1..=240))]
    scrub_hz: Option<u32>,
    /// Duration of each of the four gesture legs (defaults to 2500 ms).
    #[arg(long, requires = "scrub_trial", value_parser = clap::value_parser!(u32).range(100..=5000))]
    scrub_leg_ms: Option<u32>,
    /// Play the complete cut and report movie presentations per shot.
    #[arg(long, conflicts_with_all = ["exercise", "scrub_trial"], requires = "report")]
    playback_trial: bool,
    /// Land on an unvisited frame, then revisit it through the same SDL drag.
    #[arg(long, value_name = "FRAME", conflicts_with_all = ["exercise", "scrub_trial", "playback_trial"], requires = "report")]
    release_trial: Option<u32>,
    /// Write event-handler, seek and presentation timings on exit.
    #[arg(long)]
    report: Option<PathBuf>,
    /// Capture the player window after the exercise, including tracks and playhead.
    #[arg(long, hide = true)]
    snapshot: Option<PathBuf>,
}

struct Timings {
    input: Option<Instant>,
    landing: Option<(u32, Instant)>,
    feedback_ms: Vec<f64>,
    settled_ms: Vec<f64>,
    first_picture_ms: Option<f64>,
    opened: Instant,
}

impl Timings {
    fn new() -> Self {
        Self {
            input: None,
            landing: None,
            feedback_ms: vec![],
            settled_ms: vec![],
            first_picture_ms: None,
            opened: Instant::now(),
        }
    }
    fn input(&mut self) {
        self.input.get_or_insert_with(Instant::now);
    }
    fn land(&mut self, frame: u32) {
        self.landing = Some((frame, Instant::now()));
    }
    fn presented(&mut self, displayed: Option<u32>) {
        if let Some(at) = self.input.take()
            && self.feedback_ms.len() < 4096
        {
            self.feedback_ms.push(at.elapsed().as_secs_f64() * 1000.);
        }
        if let Some((target, at)) = self.landing
            && displayed == Some(target)
        {
            if self.settled_ms.len() < 4096 {
                self.settled_ms.push(at.elapsed().as_secs_f64() * 1000.);
            }
            self.landing = None;
        }
        if displayed.is_some() && self.first_picture_ms.is_none() {
            self.first_picture_ms = Some(self.opened.elapsed().as_secs_f64() * 1000.);
        }
    }
}

enum Loaded {
    Picture(Transport),
    Audio(PreparedSound),
}

fn prepare(snapshot: &Arc<ProjectSnapshot>) -> Result<Receiver<Result<Loaded>>> {
    let (send, recv) = mpsc::channel();
    let picture = Arc::clone(snapshot);
    let sender = send.clone();
    thread::Builder::new()
        .name("n4-open-picture".into())
        .spawn(move || {
            let _ = sender.send(picture.open_transport().map(Loaded::Picture));
        })?;
    let audio = Arc::clone(snapshot);
    thread::Builder::new()
        .name("n4-open-audio".into())
        .spawn(move || {
            let _ = send.send(audio.prepare_sound().map(Loaded::Audio));
        })?;
    Ok(recv)
}

/// Sampling happens after the first visible picture and never waits on the event loop.
fn thumbnails(snapshot: Arc<ProjectSnapshot>) -> Result<Receiver<Result<(u32, Thumbnail)>>> {
    let (send, recv) = mpsc::sync_channel(2);
    thread::Builder::new()
        .name("n4-timeline-thumbnails".into())
        .spawn(move || {
            let run = || -> Result<()> {
                let total = snapshot.document.project.duration()?;
                let count = total.min(20);
                let strip = snapshot.open_filmstrip(160)?;
                for i in 0..count {
                    let time = (((2 * u64::from(i) + 1) * u64::from(total))
                        / (2 * u64::from(count))) as u32;
                    let thumb = strip.wait(strip.request(time))?;
                    if send.send(Ok((time, thumb))).is_err() {
                        break;
                    }
                }
                Ok(())
            };
            if let Err(error) = run() {
                let _ = send.send(Err(error));
            }
        })?;
    Ok(recv)
}

enum Action {
    Quit,
    Inspector,
    Reopen,
    Toggle,
    Begin(u32),
    Drag(u32),
    Land(u32),
    Step(i32),
    Repaint,
    Ignore,
}

// SDL maps pointer events into the renderer's logical coordinate space. We set
// that space to drawable pixels for a crisp Retina preview; Layout uses window points.
#[derive(Clone, Copy)]
struct Coordinates {
    points: (u32, u32),
    pixels: (u32, u32),
}
impl Coordinates {
    fn sync(canvas: &mut WindowCanvas) -> Result<Self> {
        let points = canvas.window().size();
        let pixels = canvas.output_size().map_err(anyhow::Error::msg)?;
        if pixels.0 > 0 && pixels.1 > 0 && canvas.logical_size() != pixels {
            canvas.set_logical_size(pixels.0, pixels.1)?;
        }
        Ok(Self { points, pixels })
    }
    fn point(self, x: i32, y: i32) -> (i32, i32) {
        let scale = |v: i32, points: u32, pixels: u32| {
            (i64::from(v) * i64::from(points) / i64::from(pixels.max(1))) as i32
        };
        (
            scale(x, self.points.0, self.pixels.0),
            scale(y, self.points.1, self.pixels.1),
        )
    }
}

fn action(
    event: Event,
    coords: Coordinates,
    layout: &Layout,
    control: &Controller,
    last: u32,
) -> Action {
    use Action::*;
    match event {
        Event::Quit { .. }
        | Event::Window {
            win_event: WindowEvent::Close,
            ..
        } => Quit,
        Event::KeyDown {
            keycode: Some(key),
            keymod,
            repeat,
            ..
        } => match key {
            Keycode::Escape => Quit,
            Keycode::Q if keymod.intersects(Mod::LGUIMOD | Mod::RGUIMOD) => Quit,
            Keycode::Space if !repeat => Toggle,
            Keycode::I if !repeat => Inspector,
            Keycode::F5 if !repeat => Reopen,
            Keycode::R if !repeat && keymod.intersects(Mod::LGUIMOD | Mod::RGUIMOD) => Reopen,
            Keycode::Home => Land(0),
            Keycode::End => Land(last),
            Keycode::Left => Step(-1),
            Keycode::Right => Step(1),
            _ => Ignore,
        },
        Event::MouseButtonDown {
            mouse_btn: MouseButton::Left,
            x,
            y,
            ..
        } => match layout.hit(coords.point(x, y).0, coords.point(x, y).1) {
            Hit::Toggle => Toggle,
            Hit::Inspector => Inspector,
            Hit::Seek(n) => Begin(n),
            Hit::None => Ignore,
        },
        Event::MouseMotion { x, .. } if control.dragging() => {
            Drag(layout.frame_at(coords.point(x, 0).0))
        }
        Event::MouseButtonUp {
            mouse_btn: MouseButton::Left,
            x,
            ..
        } if control.dragging() => Land(layout.frame_at(coords.point(x, 0).0)),
        Event::Window {
            win_event: WindowEvent::FocusLost,
            ..
        } if control.dragging() => Land(control.cursor),
        Event::Window { .. } => Repaint,
        _ => Ignore,
    }
}

fn save_canvas(canvas: &WindowCanvas, path: &Path) -> Result<()> {
    let (w, h) = canvas.output_size().map_err(anyhow::Error::msg)?;
    let pixels = canvas
        .read_pixels(None, PixelFormatEnum::RGBA32)
        .map_err(anyhow::Error::msg)?;
    let mut png = png::Encoder::new(BufWriter::new(fs::File::create_new(path)?), w, h);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

fn run(args: Args) -> Result<()> {
    let open_started = Instant::now();
    for path in [&args.report, &args.snapshot].into_iter().flatten() {
        ensure!(
            !path.exists(),
            "choose a new output file: {}",
            path.display()
        );
    }
    ensure!(
        args.snapshot.is_none() || args.inspect_trial || args.exercise,
        "--snapshot requires --exercise or --inspect-trial"
    );
    let mut snapshot = ProjectSnapshot::load(&args.project, 0)?;
    let mut watch = ProjectWatch::new(&snapshot.inputs)?;
    let mut prepared_sound: Option<PreparedSound> = None;
    let mut duration = snapshot.document.project.duration()?;
    if let Some(target) = args.release_trial {
        ensure!(target < duration, "release target lies outside the project");
    }
    let scrub_range = match args.scrub_range.as_deref() {
        Some([start, end]) => {
            ensure!(
                start < end && *end <= duration,
                "scrub range must lie inside the film"
            );
            *start..*end
        }
        _ => 0..duration,
    };
    let mut fps = snapshot.document.project.fps;
    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let video = sdl.video().map_err(anyhow::Error::msg)?;
    let audio = sdl.audio().map_err(anyhow::Error::msg)?;
    let title = format!(
        "N4 Timeline — {}",
        args.project
            .file_stem()
            .unwrap_or_else(|| "Project".as_ref())
            .to_string_lossy()
    );
    let area = video.display_usable_bounds(0).map_err(anyhow::Error::msg)?;
    let width = area.width().saturating_sub(36).clamp(540, 1080);
    let height = area.height().saturating_sub(90).clamp(400, 790);
    let mut window = video
        .window(&format!("{title} — Opening…"), width, height)
        .position_centered()
        .resizable()
        .allow_highdpi()
        .build()?;
    window.set_minimum_size(480, 360)?;
    let mut canvas = window.into_canvas().accelerated().present_vsync().build()?;
    if !args.scrub_trial && args.release_trial.is_none() {
        canvas.window_mut().raise();
    }
    let window_id = canvas.window().id();
    let textures = canvas.texture_creator();
    let mut timeline = Timeline::new(&textures, &snapshot.document.project)?;
    let make_inspector = |document: &AuthoringDocument| -> Result<_> {
        Ok(InspectorHost::new(
            &textures,
            Inspector::new(document.inspection()?),
        ))
    };
    let mut inspector = args
        .inspector
        .then(|| make_inspector(&snapshot.document))
        .transpose()?;
    let mut expanded = args.inspector;
    let mut inspector_drag = false;
    let mut trial = args
        .inspect_trial
        .then(|| inspector_trial::Trial::new(args.inspect_click.clone()));
    let mut reopen_trial = (args.inspect_live_reload || args.inspect_reopen_after.is_some())
        .then(|| inspector_trial::ReopenTrial::new(args.inspect_reopen_after.as_deref()));
    let mut displayed_revision = 0;
    let mut capture_inspector = false;
    let mut movie = textures.create_texture_streaming(
        PixelFormatEnum::RGBA32,
        snapshot.document.project.size[0],
        snapshot.document.project.size[1],
    )?;
    movie.set_scale_mode(sdl2::render::ScaleMode::Linear);
    let mut control = Controller::new(duration, fps);
    if let (Some(panel), Some(placement)) = (&mut inspector, args.focus.first()) {
        panel.pane.focus(
            placement,
            args.focus.get(1).map(String::as_str),
            args.focus.get(2).map(String::as_str),
        )?;
        control.park(panel.pane.scope_start())?;
    }
    let mut events = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let event_sender = sdl.event().map_err(anyhow::Error::msg)?;
    let mouse = sdl.mouse();
    let mut exercise = args.exercise.then(Exercise::new);
    let mut scrub: Option<scrub::ScrubTrial> = None;
    let mut playback: Option<playback_trial::PlaybackTrial> = None;
    let mut release: Option<release_trial::ReleaseTrial> = None;
    let mut scrub_failure = None;
    let mut timings = Timings::new();
    let setup_before_prepare_ms = open_started.elapsed().as_secs_f64() * 1000.;
    let loaded = prepare(&snapshot)?;
    let mut reopen = reopen::Reopen::new(&args.project);
    let mut strip = None;
    let mut thumb_count = 0;
    let mut dirty = true;
    let mut thumb_started = false;
    let mut ready = false;
    let mut done = false;
    let mut finished_exercise = false;
    while !done {
        if expanded && inspector.as_ref().is_some_and(InspectorHost::pending) {
            dirty = true;
        }
        let timeout = if dirty {
            0
        } else if !ready
            || exercise.is_some()
            || args.scrub_trial
            || args.playback_trial
            || args.release_trial.is_some()
            || reopen.pending()
            || control.wants_play()
            || control.dragging()
            || control.displayed != Some(control.cursor)
            || strip.is_some()
        {
            3
        } else {
            100
        };
        // Snapshot the pending input burst before handling it. A drag never waits for the decoder.
        let first = events.wait_event_timeout(timeout);
        let coords = Coordinates::sync(&mut canvas)?;
        let layout = Layout::new(coords.points, duration);
        let mut should_reopen = watch.take_changed();
        for event in first.into_iter().chain(events.poll_iter()) {
            if args.inspect_trial && inspector_trial::unrelated_event(&event) {
                continue;
            }
            if args.release_trial.is_some() && release_trial::ReleaseTrial::unrelated_event(&event)
            {
                continue;
            }
            if scrub
                .as_mut()
                .is_some_and(|trial| trial.unrelated_event(&event))
            {
                continue;
            }
            if expanded && let Some(panel) = &mut inspector {
                let consumed = panel.event(
                    &event,
                    coords,
                    control.dragging() && !inspector_drag,
                    &video,
                );
                dirty |= panel.pending();
                if consumed {
                    continue;
                }
            }
            let op = action(event, coords, &layout, &control, duration - 1);
            if args.scrub_trial && scrub.is_none() && !matches!(op, Action::Quit | Action::Repaint)
            {
                continue;
            }
            match op {
                Action::Quit => {
                    done = true;
                    break;
                }
                Action::Ignore => continue,
                Action::Reopen => {
                    should_reopen = true;
                    continue;
                }
                Action::Inspector => {
                    if inspector.is_none() {
                        inspector = Some(make_inspector(&snapshot.document)?);
                    }
                    if inspector_drag {
                        control.park(control.cursor)?;
                        mouse.capture(false);
                        inspector_drag = false;
                    }
                    expanded = !expanded;
                    if !expanded && let Some(panel) = &mut inspector {
                        panel.suspend(&video);
                    }
                    dirty = true;
                    continue;
                }
                Action::Repaint => {
                    dirty = true;
                    continue;
                }
                _ => {
                    timings.input();
                    dirty = true;
                }
            }
            match op {
                Action::Toggle => {
                    mouse.capture(false);
                    timings.landing = None;
                    control.toggle()?;
                }
                Action::Begin(n) => {
                    mouse.capture(true);
                    timings.landing = None;
                    control.begin_drag(n)?;
                }
                Action::Drag(n) => control.drag(n)?,
                Action::Land(n) => {
                    mouse.capture(false);
                    inspector_drag = false;
                    timings.land(n);
                    control.park(n)?;
                }
                Action::Step(delta) => {
                    mouse.capture(false);
                    control.step(delta)?;
                    timings.land(control.cursor);
                }
                _ => unreachable!(),
            }
        }
        if done {
            break;
        }
        if should_reopen {
            let parked = if control.wants_play() {
                control.displayed.unwrap_or(control.cursor)
            } else {
                control.cursor
            };
            control.park(parked)?;
            inspector_drag = false;
            mouse.capture(false);
            dirty = true;
            reopen.request(&snapshot, prepared_sound.as_ref())?;
            canvas
                .window_mut()
                .set_title(&format!("{title} — Reopening…"))?;
        }
        let candidate = reopen
            .poll(control.cursor, control.frame_cache().as_ref())
            .and_then(|result| {
                let Some((change, next, picture)) = result else {
                    return Ok(None);
                };
                let project = &next.snapshot.document.project;
                let ui = (|| -> Result<_> {
                    let mut new_timeline = Timeline::new(&textures, project)?;
                    new_timeline.set_waveform(next.sound.peaks.clone());
                    let mut new_movie = textures.create_texture_streaming(
                        PixelFormatEnum::RGBA32,
                        project.size[0],
                        project.size[1],
                    )?;
                    new_movie.set_scale_mode(sdl2::render::ScaleMode::Linear);
                    new_movie.update(None, &picture.rgba, picture.size[0] as usize * 4)?;
                    let reuse_device = change.damage.audio.is_empty() && control.ready();
                    let sound = if reuse_device {
                        None
                    } else {
                        Some(Sound::open(&audio, &next.sound)?)
                    };
                    next.snapshot.inputs.check()?;
                    let sound = sound.or_else(|| control.take_sound());
                    let parked = picture.trace.request.timeline_frame;
                    let control = Controller::reopened(
                        next.inspection.duration,
                        project.fps,
                        parked,
                        next.transport,
                        sound,
                    );
                    Ok((control, new_movie, new_timeline))
                })();
                match ui {
                    Ok(ui) => Ok(Some((
                        change,
                        next.snapshot,
                        next.sound,
                        next.inspection,
                        ui,
                        picture,
                    ))),
                    Err(error) => Err(error.context(next.snapshot.inputs.clone())),
                }
            });
        match candidate {
            Ok(Some((
                change,
                next,
                next_sound,
                inspection,
                (new_control, new_movie, new_timeline),
                picture,
            ))) => {
                watch.accepted(&next.inputs);
                duration = inspection.duration;
                if let Some(panel) = &mut inspector {
                    panel.pane.reopen(inspection);
                }
                displayed_revision = change.to_revision;
                if let Some(trial) = &mut reopen_trial {
                    trial.picture(&picture.rgba);
                }
                movie = new_movie;
                control = new_control;
                timeline = new_timeline;
                fps = next.document.project.fps;
                snapshot = next;
                prepared_sound = Some(next_sound);
                strip = None;
                thumb_count = 0;
                thumb_started = false;
                ready = true;
                dirty = true;
                inspector_drag = false;
                mouse.capture(false);
                canvas.window_mut().set_title(&title)?;
                println!(
                    "Reloaded at frame {}: {}",
                    control.cursor,
                    serde_json::to_string(&change)?
                );
            }
            Err(error) => {
                watch.rejected(&error);
                eprintln!("Reopen failed: {error:#}");
                canvas
                    .window_mut()
                    .set_title(&format!("{title} — {error}"))?;
                if args.inspect_reopen_after.is_some() {
                    return Err(error.context("inspector F5 trial"));
                }
            }
            Ok(None) => {}
        }
        if !ready {
            loop {
                let result = match loaded.try_recv() {
                    Ok(message) => message?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) if control.ready() => break,
                    Err(TryRecvError::Disconnected) => bail!("project preparation stopped"),
                };
                match result {
                    Loaded::Picture(worker) => control.attach_transport(worker)?,
                    Loaded::Audio(data) => {
                        timeline.set_waveform(data.peaks.clone());
                        control.attach_sound(Sound::open(&audio, &data)?);
                        prepared_sound = Some(data);
                    }
                }
                dirty = true;
            }
            if control.ready() {
                ready = true;
                canvas.window_mut().set_title(&title)?;
            }
        }
        if let Some(receiver) = &strip {
            match Receiver::try_recv(receiver) {
                Ok(Ok((n, image))) => {
                    timeline.set_thumbnail(n, &image)?;
                    thumb_count += 1;
                    dirty = true;
                }
                Ok(Err(error)) => {
                    eprintln!("Timeline thumbnails: {error:#}");
                    strip = None;
                }
                Err(TryRecvError::Disconnected) => strip = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let was_playing = control.wants_play();
        let frame = control.poll()?;
        let new_picture = frame.is_some();
        if let Some(frame) = frame {
            displayed_revision = frame.trace.revision;
            // Say which picture of a path-traced scene this is; other pictures need no label.
            let paint = &frame.trace.graphics;
            let label = if paint.path_traced_draft > 0 {
                match n4_engine::Engine::path_trace_unavailable() {
                    None => " — Draft · tracing Final",
                    Some(_) => " — Draft · no path tracer in this build",
                }
            } else if paint.path_traced_final > 0 {
                " — Final"
            } else {
                ""
            };
            canvas.window_mut().set_title(&format!("{title}{label}"))?;
            if let Some(trial) = &mut reopen_trial {
                trial.picture(&frame.rgba);
            }
            movie.update(None, &frame.rgba, frame.size[0] as usize * 4)?;
            control.receiving(frame.trace.request.timeline_frame);
            dirty = true;
        }
        if was_playing != control.wants_play() {
            dirty = true;
        }
        if dirty {
            timeline.draw(
                &mut canvas,
                control.displayed.map(|_| &movie),
                control.cursor,
                control.displayed,
                control.wants_play(),
                if expanded {
                    InspectorHost::height(coords.points.1)
                } else {
                    0
                },
            )?;
            let panel_action = match &mut inspector {
                Some(panel) if expanded => panel.draw(&mut canvas, coords, control.cursor)?,
                _ => None,
            };
            if capture_inspector && let Some(path) = &args.snapshot {
                save_canvas(&canvas, path)?;
            }
            canvas.present();
            if let Some(trial) = &mut release {
                trial.presented(control.cursor, control.displayed, control.dragging());
            }
            if new_picture && let Some(trial) = &mut playback {
                trial.presented(
                    control.displayed.unwrap(),
                    control.cursor,
                    control.sample(),
                    control.wants_play(),
                );
            }
            if let Some(trial) = &mut scrub {
                trial.presented(control.cursor, control.displayed);
            }
            timings.presented(control.displayed);
            control.after_present(new_picture)?;
            dirty = panel_action.is_some();
            if let Some(op) = panel_action {
                timings.input();
                match op {
                    InspectorAction::Seek(n) | InspectorAction::Land(n) => {
                        mouse.capture(false);
                        inspector_drag = false;
                        timings.land(n);
                        control.park(n)?;
                    }
                    InspectorAction::Begin(n) => {
                        mouse.capture(true);
                        inspector_drag = true;
                        timings.landing = None;
                        control.begin_drag(n)?;
                    }
                    InspectorAction::Drag(n) => control.drag(n)?,
                    InspectorAction::Close => {
                        if inspector_drag {
                            control.park(control.cursor)?;
                            mouse.capture(false);
                            inspector_drag = false;
                        }
                        expanded = false;
                        if let Some(panel) = &mut inspector {
                            panel.suspend(&video);
                        }
                    }
                }
            }
            if capture_inspector {
                done = true;
                continue;
            }
        }
        if !thumb_started && control.displayed.is_some() {
            thumb_started = true;
            strip = Some(thumbnails(Arc::clone(&snapshot))?);
            println!("First picture presented; timeline accepts input.");
        }
        if args.playback_trial && playback.is_none() {
            ensure!(
                timings.opened.elapsed().as_secs() < 120,
                "playback trial timed out preparing the player"
            );
            if ready && thumb_started && strip.is_none() {
                ensure!(
                    thumb_count == duration.min(20),
                    "playback trial requires a loaded filmstrip"
                );
                ensure!(
                    control.displayed == Some(0) && control.cursor == 0,
                    "playback trial starts on frame 0"
                );
                playback = Some(playback_trial::PlaybackTrial::new(
                    control.picture_hold_samples(),
                ));
                exercise::press_space(&event_sender, window_id)?;
            }
        }
        if let Some(trial) = &playback {
            done = trial.finished(duration, control.displayed, control.wants_play())?;
        }
        if let Some(target) = args.release_trial {
            if release.is_none() {
                ensure!(
                    timings.opened.elapsed().as_secs() < 120,
                    "release trial timed out preparing the player"
                );
                if ready && thumb_started && strip.is_none() {
                    ensure!(
                        thumb_count == duration.min(20) && control.displayed == Some(0),
                        "release trial requires initial frame and filmstrip"
                    );
                    release = Some(release_trial::ReleaseTrial::new(target, &layout)?);
                }
            }
            if let Some(trial) = &mut release {
                done = trial.tick(
                    &event_sender,
                    window_id,
                    &layout,
                    control.cursor,
                    control.displayed,
                    control.dragging(),
                )?;
            }
        }
        if args.scrub_trial && ready && thumb_started && strip.is_none() && scrub.is_none() {
            ensure!(
                thumb_count == duration.min(20),
                "scrub trial requires a loaded filmstrip"
            );
            if control.cursor != scrub_range.start {
                control.park(scrub_range.start)?;
                dirty = true;
            } else if control.displayed == Some(scrub_range.start) {
                scrub = Some(scrub::ScrubTrial::start(
                    event_sender.event_sender(),
                    window_id,
                    layout,
                    scrub_range.clone(),
                    args.scrub_hz.unwrap_or(120),
                    args.scrub_leg_ms.unwrap_or(2500),
                )?);
            }
        }
        if let Some(trial) = &mut scrub {
            match trial.finished(control.dragging()) {
                Ok(finished) => done = finished,
                Err(error) => {
                    scrub_failure = Some(error);
                    done = true;
                }
            }
        }
        if let (Some(trial), Some(panel)) = (&mut trial, &inspector)
            && ready
            && thumb_started
            && strip.is_none()
            && trial.tick(
                &event_sender,
                window_id,
                &panel.pane,
                control.displayed == Some(control.cursor) && !control.dragging(),
            )?
        {
            capture_inspector = match &mut reopen_trial {
                None => true,
                Some(check) => check.tick(
                    &event_sender,
                    window_id,
                    &panel.pane,
                    control.cursor,
                    displayed_revision,
                )?,
            };
            dirty = true;
        }
        if let Some(trial) = &mut exercise {
            if !finished_exercise {
                finished_exercise = trial.tick(
                    &event_sender,
                    window_id,
                    &layout,
                    Snapshot {
                        ready,
                        duration,
                        playing: control.playing(),
                        playhead: control.cursor,
                        displayed: control.displayed,
                        fps,
                        audio_sample: control.sample(),
                    },
                )?;
            }
            if finished_exercise && strip.is_none() {
                ensure!(
                    thumb_count == duration.min(20),
                    "filmstrip incomplete: {thumb_count} thumbnails"
                );
                // Finish the exercise with the same visible UI the user will operate.
                if let Some(path) = &args.snapshot {
                    timeline.draw(
                        &mut canvas,
                        control.displayed.map(|_| &movie),
                        control.cursor,
                        control.displayed,
                        false,
                        if expanded {
                            InspectorHost::height(coords.points.1)
                        } else {
                            0
                        },
                    )?;
                    if expanded && let Some(panel) = &mut inspector {
                        panel.draw(&mut canvas, coords, control.cursor)?;
                    }
                    save_canvas(&canvas, path)?;
                }
                done = true;
            }
        }
    }
    if let Some(path) = args.report {
        let mut result = serde_json::json!({
            "exercise_completed":finished_exercise,"project_frames":duration,"fps":fps,
            "last_requested_frame":control.cursor,"last_displayed_frame":control.displayed,
            "thumbnails_visible":thumb_count,"first_picture_ms":timings.first_picture_ms,
            "project_open_to_first_picture_ms":timings.first_picture_ms.map(|ms| setup_before_prepare_ms + ms),
            "handler_to_present_ms":timings.feedback_ms,"landing_to_matching_present_ms":timings.settled_ms,
            "video_driver":video.current_video_driver(),"audio_driver":audio.current_audio_driver(),
            "boundary":"SDL event dispatch to present return; display scan-out and physical input latency are outside this measurement."
        });
        if expanded && let Some(panel) = &inspector {
            result["inspector"] = panel.pane.snapshot(control.cursor);
        }
        if args.inspect_trial {
            ensure!(
                capture_inspector && expanded,
                "inspector trial closed before the selected picture settled"
            );
            result["inspector_clicks"] = serde_json::json!(args.inspect_click);
        }
        if let Some(check) = &reopen_trial {
            result["reopen_trial"] = check.report();
        }
        if args.playback_trial {
            let trial = playback
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("playback trial closed before start"))?;
            ensure!(
                trial.finished(duration, control.displayed, control.wants_play())?,
                "playback trial closed before EOF"
            );
            result["playback_trace"] = trial.report(
                &snapshot.document.project,
                trial.elapsed_us(),
                control.picture_hold_samples(),
            )?;
        }
        if args.release_trial.is_some() {
            result["release_trial"] = release
                .as_ref()
                .context("release trial closed before start")?
                .report()?;
        }
        if let Some(mut trial) = scrub {
            if let Some(error) = &scrub_failure {
                result["scrub_failure"] = serde_json::json!(error.to_string());
            } else {
                ensure!(
                    trial.finished(control.dragging())?,
                    "scrub trial closed before completion"
                );
            }
            result["scrub_trace"] = trial.report();
        } else {
            ensure!(!args.scrub_trial, "scrub trial closed before it started");
        }
        let mut out = BufWriter::new(fs::File::create_new(path)?);
        if args.scrub_trial {
            serde_json::to_writer(&mut out, &result)?;
        } else {
            serde_json::to_writer_pretty(&mut out, &result)?;
        }
        out.flush()?;
    }
    if let Some(error) = scrub_failure {
        return Err(error);
    }
    ensure!(
        !args.exercise || finished_exercise,
        "interactive exercise closed before completion"
    );
    Ok(())
}

fn main() -> Result<()> {
    run(Args::parse())
}
