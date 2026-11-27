//! Replay clicks at the pane's actual on-screen controls through the SDL event queue.
use super::inspector::Inspector;
use anyhow::{Context, Result, ensure};
use sdl2::{
    EventSubsystem,
    event::Event,
    mouse::{MouseButton, MouseState},
};
use std::time::{Duration, Instant};

const MOUSE: u32 = 0x4e34_494e;

pub fn unrelated_event(event: &Event) -> bool {
    matches!(event, Event::MouseMotion{which,..}|Event::MouseButtonUp{which,..}|Event::MouseButtonDown{which,..} if *which != MOUSE)
}

pub struct Trial {
    targets: Vec<String>,
    next: usize,
    pressed: Option<(i32, i32)>,
    since: Instant,
}
impl Trial {
    pub fn new(targets: Vec<String>) -> Self {
        Self {
            targets,
            next: 0,
            pressed: None,
            since: Instant::now(),
        }
    }
    pub fn tick(
        &mut self,
        events: &EventSubsystem,
        window_id: u32,
        pane: &Inspector,
        settled: bool,
    ) -> Result<bool> {
        ensure!(
            self.since.elapsed() < Duration::from_secs(30),
            "inspector click did not settle at {:?}",
            self.targets.get(self.next)
        );
        if let Some((x, y)) = self.pressed.take() {
            events
                .push_event(Event::MouseButtonUp {
                    timestamp: 0,
                    window_id,
                    which: MOUSE,
                    mouse_btn: MouseButton::Left,
                    clicks: 1,
                    x,
                    y,
                })
                .map_err(anyhow::Error::msg)?;
            self.next += 1;
            return Ok(false);
        }
        if !settled {
            return Ok(false);
        }
        let Some(target) = self.targets.get(self.next) else {
            return Ok(true);
        };
        let point = pane
            .hit_point(target)
            .with_context(|| format!("inspector control {target:?} is outside the visible pane"))?;
        let (x, y) = (point.x.round() as i32, point.y.round() as i32);
        events
            .push_event(Event::MouseMotion {
                timestamp: 0,
                window_id,
                which: MOUSE,
                mousestate: MouseState::from_sdl_state(0),
                x,
                y,
                xrel: 0,
                yrel: 0,
            })
            .map_err(anyhow::Error::msg)?;
        events
            .push_event(Event::MouseButtonDown {
                timestamp: 0,
                window_id,
                which: MOUSE,
                mouse_btn: MouseButton::Left,
                clicks: 1,
                x,
                y,
            })
            .map_err(anyhow::Error::msg)?;
        self.pressed = Some((x, y));
        self.since = Instant::now();
        Ok(false)
    }
}

/// Observe a disk edit at the parked frame. The optional signal exercises manual F5.
pub struct ReopenTrial {
    signal: Option<std::path::PathBuf>,
    before: Option<(u64, serde_json::Value, Vec<u8>)>,
    latest: Vec<u8>,
    sent: bool,
    after: Option<serde_json::Value>,
    waiting_since: Option<Instant>,
}

impl ReopenTrial {
    pub fn new(signal: Option<&std::path::Path>) -> Self {
        Self {
            signal: signal.map(std::path::Path::to_owned),
            before: None,
            latest: Vec::new(),
            sent: false,
            after: None,
            waiting_since: None,
        }
    }
    pub fn picture(&mut self, rgba: &[u8]) {
        self.latest.clear();
        self.latest.extend_from_slice(rgba);
    }
    pub fn tick(
        &mut self,
        events: &EventSubsystem,
        id: u32,
        pane: &Inspector,
        cursor: u32,
        revision: u64,
    ) -> Result<bool> {
        if self.before.is_none() {
            ensure!(!self.latest.is_empty(), "missing initial inspector picture");
            self.before = Some((
                revision,
                pane.snapshot(cursor),
                std::mem::take(&mut self.latest),
            ));
            self.waiting_since = Some(Instant::now());
            if self.signal.is_some() {
                println!("Inspector F5 trial parked at frame {cursor}");
            } else {
                println!("Inspector live reload trial parked at frame {cursor}");
            }
        }
        ensure!(
            self.waiting_since.unwrap().elapsed() < Duration::from_secs(30),
            "inspector reopen trial timed out"
        );
        let (prior, view, pixels) = self.before.as_ref().unwrap();
        if !self.sent && self.signal.as_ref().is_some_and(|path| path.exists()) {
            use sdl2::keyboard::{Keycode, Mod, Scancode};
            events
                .push_event(Event::KeyDown {
                    timestamp: 0,
                    window_id: id,
                    keycode: Some(Keycode::F5),
                    scancode: Some(Scancode::F5),
                    keymod: Mod::NOMOD,
                    repeat: false,
                })
                .map_err(anyhow::Error::msg)?;
            self.sent = true;
        }
        if revision == *prior {
            return Ok(false);
        }
        ensure!(
            (self.signal.is_none() || self.sent) && revision == prior + 1,
            "unexpected displayed project revision"
        );
        ensure!(
            self.latest.len() == pixels.len(),
            "comparison requires equal output dimensions"
        );
        let after = pane.snapshot(cursor);
        for field in [
            "scope",
            "project_frame",
            "layer",
            "property",
            "selected_key",
        ] {
            ensure!(
                after[field] == view[field],
                "reopen changed {field}: {view} -> {after}"
            );
        }
        let changed = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(self.latest.as_chunks::<4>().0)
            .filter(|(a, b)| a != b)
            .count();
        self.after = Some(
            serde_json::json!({"input":if self.sent {"SDL F5"} else {"project save"},"from_revision":prior,
            "displayed_revision":revision,"parked_frame":cursor,"scope":after["scope"],
            "layer":after["layer"],"property":after["property"],"selected_key":after["selected_key"],
            "prior_value":view["current_value"],"reopened_value":after["current_value"],"changed_pixels":changed}),
        );
        Ok(true)
    }
    pub fn report(&self) -> serde_json::Value {
        self.after.clone().unwrap_or_default()
    }
}
