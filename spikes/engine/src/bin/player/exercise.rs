//! Finite regression through the native player's ordinary SDL input path.
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use n4_engine::{FrameRate, export::sample_boundary};
use sdl2::{
    EventSubsystem,
    event::Event,
    keyboard::{Keycode, Mod, Scancode},
    mouse::{MouseButton, MouseState},
};

use super::timeline::Layout;

#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    pub ready: bool,
    pub duration: u32,
    pub playing: bool,
    pub playhead: u32,
    pub displayed: Option<u32>,
    pub fps: FrameRate,
    pub audio_sample: u64,
}

impl Snapshot {
    fn parked_at(self, frame: u32) -> bool {
        !self.playing && self.playhead == frame && self.displayed == Some(frame)
    }
}

#[derive(Debug, Clone, Copy)]
enum Stage {
    Opening,
    Home,
    StepForward,
    StepBack,
    Playing,
    Pausing,
    Holding(u32),
    Resumed(u32),
    DragForward(u32),
    DragReverse(u32),
    Released(u32),
    Settled(u32),
    PlayAfterScrub(u32),
    End,
    Replay,
    Finish,
}

pub struct Exercise {
    stage: Stage,
    since: Instant,
}

impl Exercise {
    pub fn new() -> Self {
        Self {
            stage: Stage::Opening,
            since: Instant::now(),
        }
    }

    fn enter(&mut self, next: Stage) {
        self.stage = next;
        self.since = Instant::now();
    }

    /// Called once per event-loop pass after the visible state has been updated.
    pub fn tick(
        &mut self,
        events: &EventSubsystem,
        window_id: u32,
        layout: &Layout,
        state: Snapshot,
    ) -> Result<bool> {
        use Stage::*;
        let timeout = if matches!(self.stage, Opening) {
            120
        } else {
            20
        };
        ensure!(
            self.since.elapsed() < Duration::from_secs(timeout),
            "native exercise timed out in {:?}, observed {state:?}",
            self.stage
        );
        let input = Input { events, window_id };
        match self.stage {
            Opening if state.ready && state.displayed.is_some() => {
                ensure!(state.duration >= 24, "exercise needs at least 24 frames");
                input.key(Keycode::Home)?;
                self.enter(Home);
            }
            Home if state.parked_at(0) => {
                input.key(Keycode::Right)?;
                self.enter(StepForward);
            }
            StepForward if state.parked_at(1) => {
                input.key(Keycode::Left)?;
                self.enter(StepBack);
            }
            StepBack if state.parked_at(0) => {
                input.key(Keycode::Space)?;
                self.enter(Playing);
            }
            Playing if state.playing && state.displayed.is_some_and(|n| n >= 2) => {
                input.key(Keycode::Space)?;
                self.enter(Pausing);
            }
            Pausing if state.parked_at(state.playhead) => self.enter(Holding(state.playhead)),
            Holding(frame) => {
                ensure!(
                    state.parked_at(frame),
                    "pause did not hold frame {frame}: {state:?}"
                );
                if self.since.elapsed() >= Duration::from_millis(160) {
                    input.key(Keycode::Space)?;
                    self.enter(Resumed(frame));
                }
            }
            Resumed(frame) if state.playing && state.displayed.is_some_and(|n| n > frame) => {
                let point = layout.point_at(state.duration - 1 - state.duration / 4);
                input.down(point)?;
                self.enter(DragForward(layout.frame_at(point.0)));
            }
            DragForward(frame) if state.parked_at(frame) => {
                let point = layout.point_at(state.duration / 4);
                input.motion(point)?;
                self.enter(DragReverse(layout.frame_at(point.0)));
            }
            DragReverse(frame) if state.parked_at(frame) => {
                // A burst replaces several pending pointer targets in one SDL poll.
                for numerator in [6, 1, 5, 2] {
                    input.motion(layout.point_at((state.duration / 8) * numerator))?;
                }
                let landing = layout.point_at(state.duration / 2);
                input.motion(landing)?;
                input.up(landing)?;
                self.enter(Released(layout.frame_at(landing.0)));
            }
            Released(frame) if state.parked_at(frame) => self.enter(Settled(frame)),
            Settled(frame) => {
                ensure!(
                    state.parked_at(frame),
                    "stale picture after scrub to {frame}: {state:?}"
                );
                if self.since.elapsed() >= Duration::from_millis(160) {
                    input.key(Keycode::Space)?;
                    self.enter(PlayAfterScrub(frame));
                }
            }
            PlayAfterScrub(frame) if state.playing => {
                let origin = sample_boundary(state.fps, frame)?;
                // Buffering can postpone the first callback. Its clock must still
                // start from the released frame, including when it is far from zero.
                let latest = origin + (self.since.elapsed().as_secs_f64() * 48_000.0) as u64 + 1024;
                ensure!(
                    (origin..=latest).contains(&state.audio_sample),
                    "resume audio outside selected range {origin}..={latest}: {state:?}"
                );
                if state.displayed.is_some_and(|n| n > frame) {
                    input.key(Keycode::End)?;
                    self.enter(End);
                }
            }
            End if state.parked_at(state.duration - 1) => {
                input.key(Keycode::Space)?;
                self.enter(Replay);
            }
            Replay
                if state.playing
                    && state
                        .displayed
                        .is_some_and(|n| n > 0 && n < state.duration / 2) =>
            {
                input.key(Keycode::Space)?;
                self.enter(Finish);
            }
            Finish if state.parked_at(state.playhead) => return Ok(true),
            _ => {}
        }
        Ok(false)
    }
}

struct Input<'a> {
    events: &'a EventSubsystem,
    window_id: u32,
}

pub(super) fn press_space(events: &EventSubsystem, window_id: u32) -> Result<()> {
    Input { events, window_id }.key(Keycode::Space)
}

impl Input<'_> {
    fn push(&self, event: Event) -> Result<()> {
        self.events.push_event(event).map_err(anyhow::Error::msg)
    }

    fn key(&self, key: Keycode) -> Result<()> {
        let scancode = Scancode::from_keycode(key);
        self.push(Event::KeyDown {
            timestamp: 0,
            window_id: self.window_id,
            keycode: Some(key),
            scancode,
            keymod: Mod::NOMOD,
            repeat: false,
        })?;
        self.push(Event::KeyUp {
            timestamp: 0,
            window_id: self.window_id,
            keycode: Some(key),
            scancode,
            keymod: Mod::NOMOD,
            repeat: false,
        })
    }

    fn down(&self, (x, y): (i32, i32)) -> Result<()> {
        self.push(Event::MouseButtonDown {
            timestamp: 0,
            window_id: self.window_id,
            which: 0,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x,
            y,
        })
    }

    fn up(&self, (x, y): (i32, i32)) -> Result<()> {
        self.push(Event::MouseButtonUp {
            timestamp: 0,
            window_id: self.window_id,
            which: 0,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x,
            y,
        })
    }

    fn motion(&self, (x, y): (i32, i32)) -> Result<()> {
        self.push(Event::MouseMotion {
            timestamp: 0,
            window_id: self.window_id,
            which: 0,
            mousestate: MouseState::from_sdl_state(1),
            x,
            y,
            xrel: 0,
            yrel: 0,
        })
    }
}
