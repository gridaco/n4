//! Interactive frame transport. Seeking, pausing and resuming share one engine and media cache.
use crate::export::{ExportReport, write_mp4_with_progress};
use crate::{
    Engine, Frame, Project, RevisionChange,
    graphics::{PreparedSvgAssets, PreparedTextLayouts},
};
mod frame_cache;
use anyhow::{Context, Result, ensure};
pub use frame_cache::{ComposedFrameCache, FrameCacheStats};
use std::collections::VecDeque;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};

/// Two completed pictures can wait for the presentation clock.
/// The producer takes another frame only when there is room for its result.
const LOOKAHEAD: usize = 2;

struct State<T> {
    generation: u64,
    range: Range<u32>,
    ready: VecDeque<Result<T>>,
    export: Option<ExportWork>,
    export_progress: Option<Arc<ExportProgress>>,
    /// A parked frame shown as a Draft whose path-traced Final should follow.
    finish: Option<u32>,
    finishing: bool,
    shutdown: bool,
}

struct Queue<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
    /// Stops a Final in flight when the user moves on.
    interrupt: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl<T> Queue<T> {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                generation: 0,
                range: 0..0,
                ready: VecDeque::with_capacity(LOOKAHEAD),
                export: None,
                export_progress: None,
                finish: None,
                finishing: false,
                shutdown: false,
            }),
            changed: Condvar::new(),
            interrupt: Mutex::new(None),
        }
    }

    fn replace(&self, range: Range<u32>) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.range = range;
        state.ready.clear();
        state.finish = None;
        if state.finishing
            && let Some(interrupt) = &*self.interrupt.lock().unwrap()
        {
            interrupt();
        }
        self.changed.notify_one();
    }

    fn pop(&self) -> Result<Option<T>> {
        let mut state = self.state.lock().unwrap();
        let item = state.ready.pop_front();
        if item.is_some() {
            self.changed.notify_one();
        }
        item.transpose()
    }

    // Only move work that has not started. An expensive in-flight picture still
    // gets to the screen, even when the clock advances on every UI poll.
    fn skip_before(&self, due: u32, shot_ends: &[u32]) {
        let mut state = self.state.lock().unwrap();
        if state.range.is_empty() || state.range.start >= due {
            return;
        }
        let next_cut = shot_ends.partition_point(|&cut| cut < state.range.start);
        let first_protected = shot_ends.get(next_cut).copied().unwrap_or(state.range.end);
        state.range.start = due.min(first_protected).min(state.range.end - 1);
    }

    fn run<E>(
        &self,
        engine: &mut E,
        mut render: impl FnMut(&mut E, u32) -> Result<T>,
        mut export: impl FnMut(&mut E, &ExportWork) -> Result<ExportReport>,
        is_draft: impl Fn(&T) -> bool,
        mut finish: impl FnMut(&mut E, u32) -> Result<T>,
    ) {
        loop {
            let mut state = self.state.lock().unwrap();
            while !state.shutdown
                && state.export.is_none()
                && state.finish.is_none()
                && (state.range.is_empty() || state.ready.len() == LOOKAHEAD)
            {
                state = self.changed.wait(state).unwrap();
            }
            if state.shutdown {
                return;
            }
            // Parked on a Draft with nothing else to do: trace its Final and present it after.
            if state.export.is_none()
                && state.range.is_empty()
                && let Some(frame) = state.finish.take()
            {
                let generation = state.generation;
                state.finishing = true;
                drop(state);
                let result = finish(engine, frame);
                let mut state = self.state.lock().unwrap();
                state.finishing = false;
                // Moving on interrupts the trace; its error is not the new request's.
                if state.generation == generation && !state.shutdown {
                    state.ready.push_back(result);
                }
                continue;
            }
            if let Some(job) = state.export.take() {
                drop(state);
                let result = export(engine, &job);
                self.state.lock().unwrap().export_progress = None;
                let _ = job.result.send(result);
                continue;
            }
            let generation = state.generation;
            let frame = state.range.next().expect("nonempty render range");
            drop(state);
            // Decoding, GPU submission, and readback never hold the command mailbox.
            let result = render(engine, frame);
            let mut state = self.state.lock().unwrap();
            if state.generation == generation && !state.shutdown {
                if result.is_err() {
                    state.range = 0..0;
                }
                if state.range.is_empty() && result.as_ref().is_ok_and(&is_draft) {
                    state.finish = Some(frame);
                }
                state.ready.push_back(result);
            }
        }
    }

    fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.shutdown = true;
        if state.finishing
            && let Some(interrupt) = &*self.interrupt.lock().unwrap()
        {
            interrupt();
        }
        if let Some(progress) = &state.export_progress {
            progress.cancelled.store(true, Ordering::Relaxed);
        }
        state.export = None;
        self.changed.notify_one();
    }
}

#[derive(Default)]
struct ExportProgress {
    rendered: AtomicU32,
    cancelled: AtomicBool,
}

struct ExportWork {
    path: PathBuf,
    audio: Option<PathBuf>,
    progress: Arc<ExportProgress>,
    result: mpsc::SyncSender<Result<ExportReport>>,
}

impl ExportWork {
    fn run(&self, engine: &mut Engine) -> Result<ExportReport> {
        write_mp4_with_progress(engine, &self.path, self.audio.as_deref(), |n| {
            self.progress.rendered.store(n, Ordering::Relaxed);
            ensure!(
                !self.progress.cancelled.load(Ordering::Relaxed),
                "export cancelled"
            );
            Ok(())
        })
    }
}

/// One export on the transport's accepted render engine.
pub struct ExportJob {
    progress: Arc<ExportProgress>,
    result: mpsc::Receiver<Result<ExportReport>>,
}

impl ExportJob {
    pub fn rendered(&self) -> u32 {
        self.progress.rendered.load(Ordering::Relaxed)
    }

    pub fn cancellation_requested(&self) -> bool {
        self.progress.cancelled.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.progress.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Result<Option<ExportReport>> {
        match self.result.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => anyhow::bail!("export worker stopped"),
        }
    }
}

impl Drop for ExportJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct Transport {
    duration: u32,
    shot_ends: Vec<u32>,
    queue: Arc<Queue<Frame>>,
    cache: ComposedFrameCache,
    thread: Option<JoinHandle<()>>,
}

impl Transport {
    /// Wait for the initial source index and compositor before returning.
    /// Interactive commands reuse this worker for the lifetime of the open project.
    pub fn open(project: Project) -> Result<Self> {
        Self::open_revision(project, 0)
    }

    /// Publish pictures tagged with the immutable editor snapshot that owns this worker.
    pub fn open_revision(project: Project, revision: u64) -> Result<Self> {
        Self::open_with_artwork(project, revision, None, None)
    }

    pub(crate) fn open_with_artwork(
        project: Project,
        revision: u64,
        svg_assets: Option<Arc<PreparedSvgAssets>>,
        text_layouts: Option<Arc<PreparedTextLayouts>>,
    ) -> Result<Self> {
        let duration = project.duration()?;
        let shot_ends = project
            .shots
            .iter()
            .scan(0, |start, shot| {
                *start += shot.duration;
                Some(*start)
            })
            .collect();
        let queue = Arc::new(Queue::new());
        let worker_queue = Arc::clone(&queue);
        let cache = ComposedFrameCache::new(revision);
        let worker_cache = cache.clone();
        let (initialized, opened) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("n4-transport".into())
            .spawn(move || {
                let mut engine = match Engine::new_with_artwork(
                    project,
                    svg_assets.as_deref(),
                    text_layouts.as_deref(),
                )
                .and_then(|mut engine| {
                    engine.prepare_session_artwork()?;
                    Ok(engine)
                }) {
                    Ok(engine) => engine,
                    Err(error) => {
                        let _ = initialized.send(Err(error));
                        return;
                    }
                };
                engine.revision = revision;
                // Finished path-traced pictures outlive this revision and this process.
                let traces = Engine::path_trace_unavailable().is_none()
                    && engine.persist_finals_in_user_cache().is_ok();
                let cancel = engine.final_cancel();
                *worker_queue.interrupt.lock().unwrap() = Some(Box::new(move || cancel.cancel()));
                if initialized.send(Ok(())).is_ok() {
                    worker_queue.run(
                        &mut engine,
                        |engine, frame| worker_cache.render(engine, frame),
                        |engine, job| job.run(engine),
                        |picture: &Frame| traces && picture.trace.graphics.path_traced_draft > 0,
                        |engine, frame| worker_cache.finish(engine, frame),
                    );
                }
            })?;
        opened
            .recv()
            .context("transport worker stopped while opening")??;
        Ok(Self {
            duration,
            shot_ends,
            queue,
            cache,
            thread: Some(thread),
        })
    }

    pub fn frame_cache(&self) -> ComposedFrameCache {
        self.cache.clone()
    }

    /// Carry independently proven clean pictures into a validated candidate.
    pub fn inherit_frame_cache(
        &self,
        previous: &ComposedFrameCache,
        change: &RevisionChange,
    ) -> Result<()> {
        self.cache.inherit(previous, change)
    }

    /// Request exactly one picture. Older queued and in-flight results are discarded.
    pub fn seek(&self, frame: u32) -> Result<()> {
        self.check_frame(frame)?;
        self.queue.replace(frame..frame + 1);
        Ok(())
    }

    /// Return consecutive frames from the specified position to the exclusive project end.
    /// The host schedules presentation against its audio clock.
    pub fn play_from(&self, frame: u32) -> Result<()> {
        self.check_frame(frame)?;
        self.queue.replace(frame..self.duration);
        Ok(())
    }

    /// Advance pending playback work to the audio clock during overload.
    /// The opening frame of each cut and the final frame remain scheduled.
    pub fn skip_before(&self, due: u32) {
        self.queue.skip_before(due, &self.shot_ends);
    }

    pub fn starts_shot(&self, frame: u32) -> bool {
        frame == 0 || self.shot_ends.binary_search(&frame).is_ok()
    }

    /// The exclusive end of the shot containing this frame.
    pub fn shot_end(&self, frame: u32) -> u32 {
        let i = self.shot_ends.partition_point(|&cut| cut <= frame);
        self.shot_ends.get(i).copied().unwrap_or(self.duration)
    }

    pub fn pause(&self) {
        self.queue.replace(0..0);
    }

    /// Schedule delivery after any picture already in flight. The caller owns
    /// the prepared PCM spool for the lifetime of the job.
    pub fn export_mp4(&self, path: PathBuf, audio: Option<PathBuf>) -> Result<ExportJob> {
        ensure!(
            self.thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished()),
            "transport worker stopped"
        );
        let mut state = self.queue.state.lock().unwrap();
        ensure!(
            state.export_progress.is_none(),
            "export already in progress"
        );
        state.generation += 1;
        state.range = 0..0;
        state.ready.clear();
        let (result, receive) = mpsc::sync_channel(1);
        let progress = Arc::new(ExportProgress::default());
        state.export_progress = Some(Arc::clone(&progress));
        state.export = Some(ExportWork {
            path,
            audio,
            progress: Arc::clone(&progress),
            result,
        });
        self.queue.changed.notify_one();
        Ok(ExportJob {
            progress,
            result: receive,
        })
    }

    /// Poll without waiting for media, GPU work or a free place in the playback queue.
    pub fn try_next(&self) -> Result<Option<Frame>> {
        ensure!(
            self.thread.as_ref().is_some_and(|t| !t.is_finished()),
            "transport worker stopped"
        );
        self.queue.pop()
    }

    fn check_frame(&self, frame: u32) -> Result<()> {
        ensure!(
            frame < self.duration,
            "frame {frame} outside transport duration {}",
            self.duration
        );
        Ok(())
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.queue.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn next<T>(queue: &Queue<T>) -> Result<T> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(item) = queue.pop()? {
                return Ok(item);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for test picture"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn seek_and_pause_reject_an_in_flight_picture() -> Result<()> {
        let queue = Arc::new(Queue::new());
        let other = Arc::clone(&queue);
        let (started_tx, started) = mpsc::channel();
        let (allow_tx, allow) = mpsc::channel();
        let worker = thread::spawn(move || {
            other.run(
                &mut (),
                |_, n| {
                    started_tx.send(n)?;
                    allow.recv_timeout(Duration::from_secs(2))?;
                    Ok(n)
                },
                |_, _| unreachable!(),
                |_| false,
                |_, _| unreachable!(),
            )
        });
        queue.replace(4..20);
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, 4);
        queue.replace(16..17);
        queue.replace(12..13);
        allow_tx.send(())?;
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, 12);
        queue.replace(0..0);
        allow_tx.send(())?;
        queue.replace(18..20);
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, 18);
        allow_tx.send(())?;
        assert_eq!(next(&queue)?, 18);
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, 19);
        allow_tx.send(())?;
        assert_eq!(next(&queue)?, 19);
        assert!(queue.pop()?.is_none());
        queue.stop();
        worker.join().unwrap();
        Ok(())
    }

    #[test]
    fn overloaded_playback_keeps_the_inflight_picture_and_recovers_at_the_cut() -> Result<()> {
        let queue = Arc::new(Queue::new());
        let other = Arc::clone(&queue);
        let (started_tx, started) = mpsc::channel();
        let (allow_tx, allow) = mpsc::channel();
        let worker = thread::spawn(move || {
            other.run(
                &mut (),
                |_, n| {
                    started_tx.send(n)?;
                    allow.recv_timeout(Duration::from_secs(2))?;
                    Ok(n)
                },
                |_, _| unreachable!(),
                |_| false,
                |_, _| unreachable!(),
            )
        });
        let cuts = [12, 18, 24];
        queue.replace(0..24);
        let mut pending = 0;
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, pending);
        // Audio advances while two expensive pictures are already rendering.
        // Crossing the cut still schedules its opening picture. The fast final
        // shot can then play at cadence, all the way to its exact last frame.
        for (due, expected_next) in [
            (0, 1),
            (9, 9),
            (17, 12),
            (19, 18),
            (19, 19),
            (20, 20),
            (21, 21),
            (22, 22),
            (25, 23),
        ] {
            queue.skip_before(due, &cuts);
            allow_tx.send(())?;
            assert_eq!(next(&queue)?, pending);
            pending = started.recv_timeout(Duration::from_secs(2))?;
            assert_eq!(pending, expected_next);
        }
        allow_tx.send(())?;
        assert_eq!(next(&queue)?, 23);
        queue.stop();
        worker.join().unwrap();
        Ok(())
    }

    #[test]
    fn paused_buffer_is_flushed_and_errors_allow_a_new_seek() -> Result<()> {
        let queue = Arc::new(Queue::new());
        let other = Arc::clone(&queue);
        let (started_tx, started) = mpsc::channel();
        let worker = thread::spawn(move || {
            other.run(
                &mut (),
                |_, n| {
                    started_tx.send(n)?;
                    ensure!(n != 7, "failed to render test picture");
                    Ok(n)
                },
                |_, _| unreachable!(),
                |_| false,
                |_, _| unreachable!(),
            )
        });
        queue.replace(0..9);
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, 0);
        assert_eq!(started.recv_timeout(Duration::from_secs(2))?, 1);
        assert!(started.recv_timeout(Duration::from_millis(20)).is_err());
        queue.replace(0..0);
        assert!(queue.pop()?.is_none());
        queue.replace(6..10);
        assert_eq!(next(&queue)?, 6);
        assert!(next(&queue).is_err());
        queue.replace(9..10);
        assert_eq!(next(&queue)?, 9);
        queue.stop();
        worker.join().unwrap();
        Ok(())
    }

    #[test]
    fn a_parked_draft_is_followed_by_its_final_unless_the_user_moves_on() -> Result<()> {
        let queue = Arc::new(Queue::new());
        let other = Arc::clone(&queue);
        let (finishing_tx, finishing) = mpsc::channel();
        let (allow_tx, allow) = mpsc::channel::<bool>();
        let interrupted = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&interrupted);
        let interrupt_allow = allow_tx.clone();
        *queue.interrupt.lock().unwrap() = Some(Box::new(move || {
            flag.store(true, Ordering::Relaxed);
            let _ = interrupt_allow.send(false);
        }));
        let worker = thread::spawn(move || {
            other.run(
                &mut (),
                |_, n| Ok(n),
                |_, _| unreachable!(),
                // Odd frames stand for path-traced scenes without a Final.
                |n: &u32| n % 2 == 1,
                |_, n| {
                    finishing_tx.send(n)?;
                    ensure!(
                        allow.recv_timeout(Duration::from_secs(2))?,
                        "render cancelled"
                    );
                    Ok(n + 100)
                },
            )
        });
        let next = |queue: &Queue<u32>| -> Result<u32> {
            for _ in 0..200 {
                if let Some(n) = queue.pop()? {
                    return Ok(n);
                }
                thread::sleep(Duration::from_millis(5));
            }
            anyhow::bail!("no picture arrived")
        };
        // Parked on a Draft: the Draft arrives at once, the Final after its trace.
        queue.replace(5..6);
        assert_eq!(next(&queue)?, 5);
        assert_eq!(finishing.recv_timeout(Duration::from_secs(2))?, 5);
        allow_tx.send(true)?;
        assert_eq!(next(&queue)?, 105);
        // An even frame needs no Final, and a Draft passed during playback does not start one.
        queue.replace(6..7);
        assert_eq!(next(&queue)?, 6);
        queue.replace(1..3);
        assert_eq!((next(&queue)?, next(&queue)?), (1, 2));
        assert!(finishing.recv_timeout(Duration::from_millis(100)).is_err());
        // Moving on while a Final is traced interrupts it and discards its result.
        queue.replace(7..8);
        assert_eq!(next(&queue)?, 7);
        assert_eq!(finishing.recv_timeout(Duration::from_secs(2))?, 7);
        queue.replace(8..9);
        assert!(interrupted.load(Ordering::Relaxed));
        assert_eq!(next(&queue)?, 8);
        queue.stop();
        worker.join().unwrap();
        Ok(())
    }
}
