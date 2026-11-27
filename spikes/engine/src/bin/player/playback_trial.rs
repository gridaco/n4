//! Capture actual movie texture presentations during one uninterrupted native Play action.
use anyhow::{Result, ensure};
use n4_engine::{Project, export};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

// Time since Space was sent, picture frame, painted cursor, callback sample, Play intent.
type Present = (u64, u32, u32, u64, bool);

pub struct PlaybackTrial {
    start: Instant,
    initial_hold_samples: u64,
    presents: Vec<Present>,
}

impl PlaybackTrial {
    pub fn new(initial_hold_samples: u64) -> Self {
        Self {
            start: Instant::now(),
            initial_hold_samples,
            presents: vec![],
        }
    }

    pub fn presented(&mut self, frame: u32, cursor: u32, sample: u64, wants_play: bool) {
        self.presents
            .push((self.elapsed_us(), frame, cursor, sample, wants_play));
    }

    pub fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    pub fn finished(
        &self,
        duration: u32,
        displayed: Option<u32>,
        wants_play: bool,
    ) -> Result<bool> {
        let last_us = self.presents.last().map_or(0, |p| p.0);
        ensure!(
            Duration::from_micros(self.elapsed_us() - last_us) < Duration::from_secs(60),
            "playback trial stalled after {:?} (playing={wants_play})",
            self.presents.last()
        );
        Ok(!self.presents.is_empty() && displayed == Some(duration - 1) && !wants_play)
    }

    pub fn report(&self, project: &Project, end_us: u64, hold_samples: u64) -> Result<Value> {
        let mut shots = vec![];
        let mut start = 0;
        let mut first = 0;
        let origin = self.presents.first().map(|p| p.0);
        for shot in &project.shots {
            let end = start + shot.duration;
            let until = first + self.presents[first..].partition_point(|p| p.1 < end);
            let mut count = 0;
            let mut on_audio_time = 0;
            let mut by_wall_deadline = 0;
            let mut next = start;
            let mut largest_skip = 0;
            let mut longest_hold = 0;
            let mut first_play_us = None;
            let mut last_play_us = None;
            for (i, &(us, frame, _, sample, playing)) in
                self.presents[first..until].iter().enumerate()
            {
                // The single parked picture uploaded after an early audio EOF is visible
                // evidence of a jump. Keep it in the trace, outside the playback score.
                if !playing {
                    continue;
                }
                largest_skip = largest_skip.max(frame.saturating_sub(next));
                if frame < next {
                    continue;
                }
                next = frame + 1;
                count += 1;
                first_play_us.get_or_insert(us);
                last_play_us = Some(us);
                let start_sample = export::sample_boundary(project.fps, frame)?;
                let end_sample = export::sample_boundary(project.fps, next)?;
                on_audio_time += u32::from((start_sample..end_sample).contains(&sample));
                let elapsed = u128::from(us - origin.unwrap());
                by_wall_deadline += u32::from(
                    elapsed * u128::from(project.fps.0)
                        < u128::from(next) * u128::from(project.fps.1) * 1_000_000,
                );
                let next_us = self.presents.get(first + i + 1).map_or(end_us, |p| p.0);
                longest_hold = longest_hold.max(next_us.saturating_sub(us));
            }
            shots.push(json!({
                "name":shot.name, "span":[start,end], "presented_during_play":count,
                "within_audio_slot":on_audio_time, "by_wall_deadline":by_wall_deadline,
                "longest_unpresented_run":largest_skip.max(end-next),
                "longest_picture_hold_us":longest_hold,
                "first_play_present_us":first_play_us, "last_play_present_us":last_play_us
            }));
            start = end;
            first = until;
        }
        Ok(json!({
            "space_to_first_play_picture_us":origin, "space_to_parked_end_us":end_us,
            "picture_hold_audio_samples":hold_samples - self.initial_hold_samples,
            "largest_visible_frame_step":self.presents.windows(2).map(|w| w[1].1.abs_diff(w[0].1)).max(),
            "final_picture_arrived_while_parked":self.presents.last().is_some_and(|p| !p.4 && p.1 + 1 == start),
            "presents_us_frame_cursor_audio_sample_wants_play":self.presents, "shots":shots,
            "boundary":"SDL present return for a newly uploaded movie texture. Wall deadlines start at first Play picture; audio slots use the 48 kHz callback clock. Parked EOF pictures are recorded separately from playing pictures."
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_eof_park_cannot_count_as_playing_the_last_shot() -> Result<()> {
        let shot = |name| json!({"name":name,"duration":3,"background":{"color":[0,0,0]}});
        let project: Project = serde_json::from_value(json!({"fps":[10,1],"size":[16,16],
            "shots":[shot("Before"),shot("After")]}))?;
        let trial = PlaybackTrial {
            start: Instant::now(),
            initial_hold_samples: 480,
            presents: vec![
                (50_000, 0, 0, 0, true),
                (170_000, 1, 1, 4_800, true),
                (650_000, 5, 5, 28_800, false),
            ],
        };
        let result = trial.report(&project, 650_000, 960)?;
        assert_eq!(result["picture_hold_audio_samples"], 480);
        assert_eq!(result["largest_visible_frame_step"], 4);
        assert_eq!(result["final_picture_arrived_while_parked"], true);
        assert_eq!(result["shots"][0]["longest_picture_hold_us"], 480_000);
        assert_eq!(result["shots"][0]["within_audio_slot"], 2);
        assert_eq!(result["shots"][1]["presented_during_play"], 0);
        assert_eq!(result["shots"][1]["longest_unpresented_run"], 3);
        assert!(result["shots"][1]["last_play_present_us"].is_null());
        Ok(())
    }
}
