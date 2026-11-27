//! Wall-clock input replay. The pointer producer never waits for a picture or UI poll.
use super::timeline::Layout;
use anyhow::{Result, ensure};
use sdl2::{
    event::{Event, EventSender, WindowEvent},
    mouse::{MouseButton, MouseState},
};
use serde_json::{Value, json};
use std::{
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const LEGS: usize = 4;
// A private SDL mouse identity for the injected gesture. Real device events
// and focus changes can occur while this short-lived test window is open.
const REPLAY_MOUSE: u32 = 0x4e34_5342;

// Monotonic microseconds, actual quantized pointer frame.
type Input = (u64, u32);

pub struct ScrubTrial {
    start: Instant,
    hz: u32,
    leg_ticks: u32,
    producer: Option<JoinHandle<Result<Vec<Input>>>>,
    inputs: Vec<Input>,
    // Monotonic microseconds, painted playhead frame, painted movie frame.
    presents: Vec<(u64, u32, u32)>,
    unrelated_mouse_events: u32,
    focus_losses: u32,
}

impl ScrubTrial {
    pub fn start(
        events: EventSender,
        window: u32,
        layout: Layout,
        range: std::ops::Range<u32>,
        hz: u32,
        leg_ms: u32,
    ) -> Result<Self> {
        ensure!(range.len() >= 24, "scrub trial requires at least 24 frames");
        ensure!((1..=240).contains(&hz), "pointer rate must be 1–240 Hz");
        let leg_ticks = (hz * leg_ms).div_ceil(1000);
        let end_tick = leg_ticks * LEGS as u32;
        let start = Instant::now();
        let initial_frame = range.start;
        let producer = thread::Builder::new()
            .name("n4-scrub-input".into())
            .spawn(move || {
                let span = range.end - range.start - 1;
                let points =
                    [0, span / 12, 0, span / 5 * 4, span / 5].map(|offset| range.start + offset);
                let mut inputs = Vec::new();
                for tick in 0..=end_tick {
                    let until = start + Duration::from_secs_f64(f64::from(tick) / f64::from(hz));
                    thread::sleep(until.saturating_duration_since(Instant::now()));
                    let leg = ((tick / leg_ticks) as usize).min(LEGS - 1);
                    let fraction = f64::from(tick - leg as u32 * leg_ticks) / f64::from(leg_ticks);
                    let frame = f64::from(points[leg])
                        + (f64::from(points[leg + 1]) - f64::from(points[leg])) * fraction;
                    let (x, y) = layout.point_at(frame.round() as u32);
                    let event = match tick {
                        0 => Event::MouseButtonDown {
                            timestamp: 0,
                            window_id: window,
                            which: REPLAY_MOUSE,
                            mouse_btn: MouseButton::Left,
                            clicks: 1,
                            x,
                            y,
                        },
                        last if last == end_tick => Event::MouseButtonUp {
                            timestamp: 0,
                            window_id: window,
                            which: REPLAY_MOUSE,
                            mouse_btn: MouseButton::Left,
                            clicks: 1,
                            x,
                            y,
                        },
                        _ => Event::MouseMotion {
                            timestamp: 0,
                            window_id: window,
                            which: REPLAY_MOUSE,
                            mousestate: MouseState::from_sdl_state(1),
                            x,
                            y,
                            xrel: 0,
                            yrel: 0,
                        },
                    };
                    let sent = start.elapsed().as_micros() as u64;
                    events.push_event(event).map_err(anyhow::Error::msg)?;
                    inputs.push((sent, layout.frame_at(x)));
                }
                Ok(inputs)
            })?;
        Ok(Self {
            start,
            hz,
            leg_ticks,
            producer: Some(producer),
            inputs: vec![],
            presents: vec![(0, initial_frame, initial_frame)],
            unrelated_mouse_events: 0,
            focus_losses: 0,
        })
    }

    /// Keep the ordinary input handler, transport and display path under a
    /// single pointer's control for the duration of the injected gesture.
    pub fn unrelated_event(&mut self, event: &Event) -> bool {
        match event {
            Event::MouseButtonDown { which, .. }
            | Event::MouseButtonUp { which, .. }
            | Event::MouseMotion { which, .. }
                if *which != REPLAY_MOUSE =>
            {
                self.unrelated_mouse_events += 1;
                true
            }
            Event::Window {
                win_event: WindowEvent::FocusLost,
                ..
            } => {
                self.focus_losses += 1;
                true
            }
            _ => false,
        }
    }

    pub fn presented(&mut self, cursor: u32, picture: Option<u32>) {
        if let Some(frame) = picture {
            self.presents
                .push((self.start.elapsed().as_micros() as u64, cursor, frame));
        }
    }

    pub fn finished(&mut self, dragging: bool) -> Result<bool> {
        ensure!(
            self.start.elapsed() < Duration::from_secs(30),
            "scrub release failed to settle: dragging={dragging}, last input={:?}, last present={:?}, input count={}, present count={}",
            self.inputs.last(),
            self.presents.last(),
            self.inputs.len(),
            self.presents.len(),
        );
        if let Some(producer) = self.producer.take_if(|producer| producer.is_finished()) {
            self.inputs = producer
                .join()
                .map_err(|_| anyhow::anyhow!("input producer panicked"))??;
        }
        Ok(!dragging
            && matches!(
                (self.inputs.last(), self.presents.last()),
                (Some(&(release, target)), Some(&(at, cursor, picture)))
                    if at >= release && target == cursor && cursor == picture
            ))
    }

    pub fn report(self) -> Value {
        json!({
            "pointer_hz": self.hz,
            "ticks_per_leg": self.leg_ticks,
            "unrelated_mouse_events": self.unrelated_mouse_events,
            "focus_losses": self.focus_losses,
            "inputs_us_frame": self.inputs,
            "presents_us_cursor_picture": self.presents,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controlled_drag_ignores_unrelated_input_and_preserves_quit() {
        let mut trial = ScrubTrial {
            start: Instant::now(),
            hz: 120,
            leg_ticks: 300,
            producer: None,
            inputs: vec![],
            presents: vec![],
            unrelated_mouse_events: 0,
            focus_losses: 0,
        };
        let up = |which| Event::MouseButtonUp {
            timestamp: 0,
            window_id: 1,
            which,
            mouse_btn: MouseButton::Left,
            clicks: 1,
            x: 20,
            y: 10,
        };
        assert!(trial.unrelated_event(&up(0)));
        assert!(!trial.unrelated_event(&up(REPLAY_MOUSE)));
        assert!(trial.unrelated_event(&Event::Window {
            timestamp: 0,
            window_id: 1,
            win_event: WindowEvent::FocusLost,
        }));
        assert!(!trial.unrelated_event(&Event::Quit { timestamp: 0 }));
        assert_eq!((trial.unrelated_mouse_events, trial.focus_losses), (1, 1));
    }
}
