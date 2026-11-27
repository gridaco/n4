//! Latest-request preview. At most one frame renders, one request waits and one result is retained.
use crate::{
    Engine, Frame, Project,
    graphics::{PreparedSvgAssets, PreparedTextLayouts},
};
use anyhow::{Context, Result, anyhow, ensure};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};

#[cfg(feature = "host-audio")]
mod clip;
#[cfg(feature = "host-audio")]
pub(crate) use clip::ClipThumbnails;

struct State<T, Request> {
    ticket: u64,
    delivered: u64,
    pending: Option<Request>,
    ready: Option<Result<T>>,
    stop: bool,
}

struct Mailbox<T, Request> {
    state: Mutex<State<T, Request>>,
    changed: Condvar,
}

/// One current result per consumer. Picture and source thumbnail workers use separate queues.
pub struct Latest<T, Request = (u32, bool)> {
    mailbox: Arc<Mailbox<T, Request>>,
    thread: Option<JoinHandle<()>>,
}

pub type Preview = Latest<Frame>;

impl Preview {
    pub fn open(project: Project) -> Result<Self> {
        Self::open_with_proxies(project, Default::default())
    }

    pub fn open_with_proxies(project: Project, proxies: crate::proxy::ProxyMap) -> Result<Self> {
        Self::spawn("n4-preview", move || {
            let mut engine = Engine::new(project)?;
            crate::proxy::install(&mut engine.sources, &proxies, engine.project.fps)?;
            Ok(move |(frame, cold)| {
                if cold {
                    engine.reader = Default::default();
                    engine.paragraph = None;
                    // Drop mutable painting and device caches. The pinned scene must
                    // still draw if its original USD file has since been moved.
                    let meshes = std::mem::take(&mut engine.graphics.meshes);
                    engine.graphics = Default::default();
                    engine.graphics.meshes = meshes;
                }
                let mut result = engine.render(frame)?;
                result.trace.proxy = result
                    .trace
                    .request
                    .source
                    .as_ref()
                    .and_then(|p| proxies.get(p))
                    .cloned();
                for layer in &mut result.trace.request.video_layers {
                    layer.proxy = proxies.get(&layer.source).cloned();
                }
                Ok(result)
            })
        })
    }

    /// Clear mutable picture and decoder caches. Immutable project indices and OS file pages survive.
    pub fn cold_request(&self, frame: u32) -> u64 {
        self.submit((frame, true))
    }
}

pub struct Thumbnail {
    pub size: [u32; 2],
    pub rgba: Arc<Vec<u8>>,
    /// Media identity for a source thumbnail. Composition thumbnails have their own shot clock.
    pub source: Option<std::path::PathBuf>,
    /// Presentation ordinal for media, shot-local frame for a composition.
    pub source_frame: u32,
    pub proxy: Option<std::path::PathBuf>,
}

pub type Filmstrip = Latest<Thumbnail>;

impl Filmstrip {
    /// Media shots use source thumbnails. Authored compositions sample their finished picture.
    pub fn open(project: Project, width: u32) -> Result<Self> {
        Self::open_with_proxies(project, width, Default::default())
    }

    pub fn open_with_proxies(
        project: Project,
        width: u32,
        proxies: crate::proxy::ProxyMap,
    ) -> Result<Self> {
        Self::open_with_artwork(project, width, proxies, None, None)
    }

    pub(crate) fn open_with_artwork(
        project: Project,
        width: u32,
        proxies: crate::proxy::ProxyMap,
        svg_assets: Option<Arc<PreparedSvgAssets>>,
        text_layouts: Option<Arc<PreparedTextLayouts>>,
    ) -> Result<Self> {
        project.validate()?;
        ensure!(
            width > 0 && width <= 512,
            "thumbnail width must be in 1..=512"
        );
        Self::spawn("n4-filmstrip", move || {
            let mut project = project;
            let [w, h] = project.size;
            let height =
                u32::try_from((u64::from(h) * u64::from(width) + u64::from(w) / 2) / u64::from(w))?
                    .max(1);
            ensure!(height <= 1024, "thumbnail height exceeds 1024 pixels");
            let size = [width, height];
            // Media source indices can decode directly to the strip size. Authored path
            // coordinates, filter radii and text layout require the original canvas.
            project.size = size;
            let mut sources = crate::prepare_sources(&project, &Default::default(), false)?;
            project.size = [w, h];
            // The strip follows the primary cut, including when the caller also
            // supplied a valid preview proxy for an inset-only source.
            let layer_clips = project.video_clips();
            for path in proxies.keys() {
                ensure!(
                    sources.contains_key(&(path.clone(), size))
                        || layer_clips.iter().any(|clip| &clip.source == path),
                    "no project source for proxy key {}",
                    path.display()
                );
            }
            let mut source_proxies = proxies.clone();
            source_proxies.retain(|path, _| sources.contains_key(&(path.clone(), size)));
            crate::proxy::install(&mut sources, &source_proxies, project.fps)?;
            let mut reader = crate::media::VideoReader::default();
            let mut compositor = None;
            Ok(move |(frame, _)| {
                let (shot, local) = project.shot_at(frame)?;
                if shot.camera_at(local).is_some()
                    || shot.background.is_some()
                    || !shot.graphics.is_empty()
                    || project.active_overlays(frame).next().is_some()
                {
                    if compositor.is_none() {
                        let mut engine = Engine::new_with_artwork(
                            project.clone(),
                            svg_assets.as_deref(),
                            text_layouts.as_deref(),
                        )?;
                        crate::proxy::install(&mut engine.sources, &proxies, project.fps)?;
                        compositor = Some(engine);
                    }
                    let picture = compositor.as_mut().unwrap().render(frame)?;
                    return Ok(Thumbnail {
                        size,
                        rgba: Arc::new(resample(picture, size)?),
                        source: None,
                        source_frame: local,
                        proxy: None,
                    });
                }
                let path = shot.source.as_ref().context("missing media shot source")?;
                let source = sources
                    .get(&(path.clone(), size))
                    .context("missing thumbnail source")?;
                let ordinal = source.resolve(shot.source_in + local)?;
                let (rgba, _) = reader.read(source, ordinal)?;
                Ok(Thumbnail {
                    size,
                    rgba,
                    proxy: proxies.get(path).cloned(),
                    source: Some(path.clone()),
                    source_frame: ordinal,
                })
            })
        })
    }
}

fn resample(picture: Frame, size: [u32; 2]) -> Result<Vec<u8>> {
    use skia_safe::{
        AlphaType, ColorType, FilterMode, ImageInfo, MipmapMode, Pixmap, SamplingOptions,
    };
    let mut rgba = Arc::unwrap_or_clone(picture.rgba);
    if picture.size == size {
        return Ok(rgba);
    }
    let info = |[w, h]: [u32; 2]| {
        ImageInfo::new(
            (w as i32, h as i32),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        )
    };
    let src_info = info(picture.size);
    let dst_info = info(size);
    let mut pixels = vec![0; dst_info.compute_min_byte_size()];
    let src = Pixmap::new(&src_info, &mut rgba, src_info.min_row_bytes())
        .context("invalid composition pixels")?;
    let mut dst = Pixmap::new(&dst_info, &mut pixels, dst_info.min_row_bytes())
        .context("invalid thumbnail canvas")?;
    ensure!(
        src.scale_pixels(
            &mut dst,
            SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear)
        ),
        "could not downsample composition thumbnail"
    );
    Ok(pixels)
}

impl<T: Send + 'static, Request: Send + 'static> Latest<T, Request> {
    fn spawn<R>(name: &str, open: impl FnOnce() -> Result<R> + Send + 'static) -> Result<Self>
    where
        R: FnMut(Request) -> Result<T>,
    {
        let mailbox = Arc::new(Mailbox {
            state: Mutex::new(State {
                ticket: 0,
                delivered: 0,
                pending: None,
                ready: None,
                stop: false,
            }),
            changed: Condvar::new(),
        });
        let worker = mailbox.clone();
        let (initialized, opened) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name(name.into()).spawn(move || {
            let mut render = match open() {
                Ok(render) => render,
                Err(error) => {
                    let _ = initialized.send(Err(error));
                    return;
                }
            };
            if initialized.send(Ok(())).is_err() {
                return;
            }
            loop {
                let mut state = worker.state.lock().unwrap();
                while !state.stop && state.pending.is_none() {
                    state = worker.changed.wait(state).unwrap();
                }
                if state.stop {
                    return;
                }
                let ticket = state.ticket;
                let request = state.pending.take().unwrap();
                drop(state);
                let result = render(request);
                let mut state = worker.state.lock().unwrap();
                if state.ticket == ticket && !state.stop {
                    state.ready = Some(result);
                    worker.changed.notify_all();
                }
            }
        })?;
        opened.recv()??;
        Ok(Self {
            mailbox,
            thread: Some(thread),
        })
    }

    fn submit(&self, request: Request) -> u64 {
        let mut state = self.mailbox.state.lock().unwrap();
        state.ticket += 1;
        state.pending = Some(request);
        state.ready = None;
        self.mailbox.changed.notify_all();
        state.ticket
    }

    /// Check a thumbnail without blocking presentation or transport commands.
    pub fn try_take(&self, ticket: u64) -> Result<Option<T>> {
        let mut state = self.mailbox.state.lock().unwrap();
        ensure!(ticket == state.ticket && ticket != 0, "superseded ticket");
        ensure!(state.delivered != ticket, "ticket already consumed");
        if let Some(ready) = state.ready.take() {
            state.delivered = ticket;
            self.mailbox.changed.notify_all();
            return ready.map(Some);
        }
        ensure!(!state.stop, "closed preview");
        Ok(None)
    }

    /// Single-consumer mailbox. A superseded ticket can never return another ticket's pixels.
    pub fn wait(&self, ticket: u64) -> Result<T> {
        let mut state = self.mailbox.state.lock().unwrap();
        loop {
            ensure!(
                ticket == state.ticket && ticket != 0,
                "superseded preview ticket {ticket}"
            );
            ensure!(
                state.delivered != ticket,
                "preview ticket {ticket} already consumed"
            );
            if let Some(ready) = state.ready.take() {
                state.delivered = ticket;
                self.mailbox.changed.notify_all();
                return ready;
            }
            ensure!(!state.stop, "closed preview");
            let (next, timeout) = self
                .mailbox
                .changed
                .wait_timeout(state, std::time::Duration::from_secs(30))
                .unwrap();
            state = next;
            if timeout.timed_out() && state.ready.is_none() {
                return Err(anyhow!("preview frame was unavailable for 30 seconds"));
            }
        }
    }
}

impl<T: Send + 'static> Latest<T> {
    pub fn request(&self, frame: u32) -> u64 {
        self.submit((frame, false))
    }
}

impl<T, Request> Drop for Latest<T, Request> {
    fn drop(&mut self) {
        self.mailbox.state.lock().unwrap().stop = true;
        self.mailbox.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Continuous playback keeps decoding in order while the display follows the sound
/// clock. A missed deadline must not turn the next picture into a random-access seek.
pub struct Playback {
    frames: Option<mpsc::Receiver<Result<Frame>>>,
    worker: Option<JoinHandle<()>>,
}

impl Playback {
    pub fn open(project: Project, count: u32) -> Result<Self> {
        ensure!(
            count > 0 && count <= project.duration()?,
            "invalid playback range"
        );
        let (send, frames) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("n4-playback".into())
            .spawn(move || {
                let mut engine = match Engine::new(project) {
                    Ok(engine) => engine,
                    Err(e) => {
                        let _ = send.send(Err(e));
                        return;
                    }
                };
                for n in 0..count {
                    let result = engine.render(n);
                    let failed = result.is_err();
                    if send.send(result).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            frames: Some(frames),
            worker: Some(worker),
        })
    }

    pub fn receive(&self) -> Result<Frame> {
        self.frames
            .as_ref()
            .context("playback closed")?
            .recv_timeout(std::time::Duration::from_secs(120))?
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.frames.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
