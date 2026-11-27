//! Two independent release measurements in one native window. The first lands
//! on an unvisited target; the second revisits it through the same gesture.
use super::timeline::Layout;
use anyhow::{Result, ensure};
use sdl2::{
    EventSubsystem,
    event::{Event, WindowEvent},
    mouse::{MouseButton, MouseState},
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const MOUSE: u32 = 0x4e34_5352;

enum Gesture {
    Begin,
    Moving,
    Released(Instant),
}

pub struct ReleaseTrial {
    start: Instant,
    requested: u32,
    target: u32,
    lead: u32,
    state: Gesture,
    latency_ms: Vec<f64>,
}

impl ReleaseTrial {
    pub fn new(requested: u32, layout: &Layout) -> Result<Self> {
        let reachable = |frame| layout.frame_at(layout.point_at(frame).0);
        let target = reachable(requested);
        let lead = reachable(requested / 4);
        ensure!(
            lead > 0 && lead < target,
            "choose a release target separated from frame zero and its drag lead-in"
        );
        Ok(Self {
            start: Instant::now(),
            requested,
            target,
            lead,
            state: Gesture::Begin,
            latency_ms: vec![],
        })
    }

    pub fn unrelated_event(event: &Event) -> bool {
        matches!(event, Event::MouseButtonDown { which, .. } | Event::MouseButtonUp { which, .. }
            | Event::MouseMotion { which, .. } if *which != MOUSE)
            || matches!(
                event,
                Event::Window {
                    win_event: WindowEvent::FocusLost,
                    ..
                }
            )
    }

    /// Called immediately after SDL's normal presentation returns.
    pub fn presented(&mut self, cursor: u32, picture: Option<u32>, dragging: bool) {
        if let Gesture::Released(sent) = self.state
            && !dragging
            && cursor == self.target
            && picture == Some(self.target)
        {
            self.latency_ms.push(sent.elapsed().as_secs_f64() * 1000.);
            self.state = Gesture::Begin;
        }
    }

    pub fn tick(
        &mut self,
        events: &EventSubsystem,
        window: u32,
        layout: &Layout,
        cursor: u32,
        picture: Option<u32>,
        dragging: bool,
    ) -> Result<bool> {
        if self.latency_ms.len() == 2 {
            return Ok(true);
        }
        ensure!(
            self.start.elapsed() < Duration::from_secs(30),
            "release trial timed out at target {}, cursor {cursor}, picture {picture:?}",
            self.target
        );
        let send = |ev| events.push_event(ev).map_err(anyhow::Error::msg);
        match self.state {
            Gesture::Begin => {
                let (x, y) = layout.point_at(0);
                send(Event::MouseButtonDown {
                    timestamp: 0,
                    window_id: window,
                    which: MOUSE,
                    mouse_btn: MouseButton::Left,
                    clicks: 1,
                    x,
                    y,
                })?;
                let (x, y) = layout.point_at(self.lead);
                send(Event::MouseMotion {
                    timestamp: 0,
                    window_id: window,
                    which: MOUSE,
                    mousestate: MouseState::from_sdl_state(1),
                    x,
                    y,
                    xrel: 0,
                    yrel: 0,
                })?;
                self.state = Gesture::Moving;
            }
            Gesture::Moving if dragging && cursor == self.lead && picture == Some(self.lead) => {
                let (x, y) = layout.point_at(self.target);
                self.state = Gesture::Released(Instant::now());
                send(Event::MouseButtonUp {
                    timestamp: 0,
                    window_id: window,
                    which: MOUSE,
                    mouse_btn: MouseButton::Left,
                    clicks: 1,
                    x,
                    y,
                })?;
            }
            _ => {}
        }
        Ok(false)
    }

    pub fn report(&self) -> Result<Value> {
        ensure!(
            self.latency_ms.len() == 2,
            "release trial closed before both targets were presented"
        );
        Ok(
            json!({"requested_frame":self.requested,"landed_frame":self.target,"lead_in_frame":self.lead,
            "unvisited_target_ms":self.latency_ms[0],"revisit_ms":self.latency_ms[1],
            "boundary":"SDL mouse-up injection at an unvisited landing position to return from the first matching movie presentation; both trials first present a drag lead-in away from the target."}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_requires_the_exact_presented_movie_frame() {
        let layout = Layout::new((1080, 720), 1440);
        let mut trial = ReleaseTrial::new(301, &layout).unwrap();
        trial.state = Gesture::Released(Instant::now());
        let target = trial.target;
        for (cursor, picture, dragging) in [
            (target, None, false),
            (target - 1, Some(target), false),
            (target, Some(target - 1), false),
            (target, Some(target), true),
        ] {
            trial.presented(cursor, picture, dragging);
        }
        assert!(trial.latency_ms.is_empty());
        trial.presented(target, Some(target), false);
        trial.presented(target, Some(target), false);
        assert_eq!(trial.latency_ms.len(), 1);
    }
}
