//! The UI thread polls the worker. Displaying intermediate scrub results never blocks pointer input.
use super::sound::Sound;
use crate::{
    Frame, FrameRate, export,
    transport::{ComposedFrameCache, ExportJob, Transport},
};
use anyhow::Result;
use std::path::PathBuf;

// Mild picture overload drops frames against the sound clock. A longer stall
// holds the mix instead of letting the rest of the edit elapse unseen.
const MAX_AUDIO_LEAD: u64 = export::SAMPLE_RATE as u64 / 4;

fn due_frame(fps: FrameRate, sample: u64, duration: u32) -> u32 {
    fps.frame_at_sample(sample, duration)
}

fn picture_limit(fps: FrameRate, shown: u32, shot_end: u32, duration: u32) -> Result<u64> {
    let position = export::sample_boundary(fps, shown)?;
    let two_frames = export::sample_boundary(fps, shown.saturating_add(2).min(duration))?;
    // Allow the device one frame across a cut for normal presentation jitter.
    let after_cut = export::sample_boundary(fps, shot_end.saturating_add(1).min(duration))?;
    Ok(position
        .saturating_add(MAX_AUDIO_LEAD)
        .max(two_frames)
        .min(after_cut))
}

#[derive(PartialEq, Eq)]
enum Mode {
    Parked,
    Dragging,
    Starting,
    Playing,
}

pub struct Controller {
    duration: u32,
    fps: FrameRate,
    mode: Mode,
    pub cursor: u32,
    pub displayed: Option<u32>,
    transport: Option<Transport>,
    sound: Option<Sound>,
    seeking: Option<u32>,
    upcoming: Option<Frame>,
    /// The parked picture is a Draft of a path-traced scene and its Final may still arrive.
    awaiting_final: bool,
}

impl Controller {
    pub fn new(duration: u32, fps: FrameRate) -> Self {
        Self {
            duration,
            fps,
            mode: Mode::Parked,
            cursor: 0,
            displayed: None,
            transport: None,
            sound: None,
            seeking: None,
            upcoming: None,
            awaiting_final: false,
        }
    }
    pub fn reopened(
        duration: u32,
        fps: FrameRate,
        cursor: u32,
        worker: Transport,
        sound: Option<Sound>,
    ) -> Self {
        Self {
            cursor,
            displayed: Some(cursor),
            transport: Some(worker),
            sound,
            ..Self::new(duration, fps)
        }
    }

    pub fn ready(&self) -> bool {
        self.transport.is_some() && self.sound.is_some()
    }
    pub fn playing(&self) -> bool {
        self.mode == Mode::Playing
    }
    pub fn wants_play(&self) -> bool {
        matches!(self.mode, Mode::Starting | Mode::Playing)
    }
    pub fn dragging(&self) -> bool {
        self.mode == Mode::Dragging
    }
    pub fn sample(&self) -> u64 {
        self.sound.as_ref().map_or(0, Sound::current_sample)
    }
    pub fn picture_hold_samples(&self) -> u64 {
        self.sound.as_ref().map_or(0, Sound::picture_hold_samples)
    }

    pub fn attach_transport(&mut self, worker: Transport) -> Result<()> {
        self.transport = Some(worker);
        if self.mode == Mode::Starting {
            self.play_command()
        } else {
            self.seek(self.cursor)
        }
    }
    pub fn frame_cache(&self) -> Option<ComposedFrameCache> {
        self.transport.as_ref().map(Transport::frame_cache)
    }

    /// Detach the parked device only when the successor has passed validation.
    pub fn take_sound(&mut self) -> Option<Sound> {
        let mut sound = self.sound.take()?;
        sound.pause();
        Some(sound)
    }

    pub fn attach_sound(&mut self, sound: Sound) {
        self.sound = Some(sound);
    }

    fn pause_output(&mut self) {
        if let Some(sound) = &mut self.sound {
            sound.pause();
        }
        if let Some(worker) = &self.transport {
            worker.pause();
        }
        self.upcoming = None;
        self.seeking = None;
    }
    fn seek(&mut self, frame: u32) -> Result<()> {
        if let Some(worker) = &self.transport {
            worker.seek(frame)?;
            self.seeking = Some(frame);
            self.awaiting_final = false;
        }
        Ok(())
    }
    fn play_command(&mut self) -> Result<()> {
        if let Some(worker) = &self.transport {
            worker.play_from(self.cursor)?;
        }
        Ok(())
    }
    /// A remote client can request pixels even when the cursor is already there.
    pub fn seek_exact(&mut self, frame: u32) -> Result<()> {
        self.pause_output();
        self.mode = Mode::Parked;
        self.cursor = frame;
        self.seek(frame)
    }

    pub fn play_at(&mut self, frame: u32) -> Result<()> {
        self.pause_output();
        self.cursor = frame;
        self.mode = Mode::Starting;
        self.play_command()
    }

    pub fn pause(&mut self) {
        self.pause_output();
        self.mode = Mode::Parked;
        self.cursor = self.displayed.unwrap_or(self.cursor);
    }

    pub(crate) fn export_mp4(
        &mut self,
        path: PathBuf,
        audio: Option<PathBuf>,
    ) -> Result<ExportJob> {
        self.pause();
        self.transport
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("transport is not open"))?
            .export_mp4(path, audio)
    }

    pub fn toggle(&mut self) -> Result<()> {
        if self.wants_play() {
            self.park(self.displayed.unwrap_or(self.cursor))
        } else {
            self.pause_output();
            if self.cursor == self.duration - 1 {
                self.cursor = 0;
            }
            self.mode = Mode::Starting;
            self.play_command()
        }
    }
    /// The final landing frame supersedes any outstanding intermediate scrub.
    pub fn park(&mut self, target: u32) -> Result<()> {
        self.pause_output();
        self.mode = Mode::Parked;
        self.cursor = target.min(self.duration - 1);
        if self.displayed != Some(self.cursor) {
            self.seek(self.cursor)?;
        }
        Ok(())
    }
    pub fn step(&mut self, delta: i32) -> Result<()> {
        let current = if self.playing() {
            self.displayed.unwrap_or(self.cursor)
        } else {
            self.cursor
        };
        self.park(current.saturating_add_signed(delta).min(self.duration - 1))
    }
    pub fn begin_drag(&mut self, target: u32) -> Result<()> {
        self.pause_output();
        self.mode = Mode::Dragging;
        self.drag(target)
    }
    pub fn drag(&mut self, target: u32) -> Result<()> {
        self.cursor = target.min(self.duration - 1);
        if self.seeking.is_none() && self.displayed != Some(self.cursor) {
            self.seek(self.cursor)?;
        }
        Ok(())
    }

    /// Retain one future picture and use the audio device clock to choose when it is due.
    pub fn poll(&mut self) -> Result<Option<Frame>> {
        if let Some(sound) = &self.sound {
            sound.check()?;
        }
        let Some(worker) = &self.transport else {
            return Ok(None);
        };
        match self.mode {
            Mode::Parked | Mode::Dragging => {
                if self.seeking.is_none() && !self.awaiting_final {
                    return Ok(None);
                }
                let Some(frame) = worker.try_next()? else {
                    return Ok(None);
                };
                self.seeking = None;
                // The transport follows a parked Draft with its Final for the same frame.
                self.awaiting_final =
                    !self.dragging() && frame.trace.graphics.path_traced_draft > 0;
                if self.dragging() && self.cursor != frame.trace.request.timeline_frame {
                    self.seek(self.cursor)?;
                }
                Ok(Some(frame))
            }
            Mode::Starting => {
                if self.sound.is_none() {
                    return Ok(None);
                }
                worker.try_next()
            }
            Mode::Playing => {
                let sample = self.sample();
                if self.displayed == Some(self.duration - 1)
                    && sample >= export::sample_boundary(self.fps, self.duration)?
                {
                    self.park(self.duration - 1)?;
                    return Ok(None);
                }
                let target = due_frame(self.fps, sample, self.duration);
                worker.skip_before(target);
                let mut due = None;
                loop {
                    if self.upcoming.is_none() {
                        self.upcoming = worker.try_next()?;
                    }
                    let Some(next) = self.upcoming.as_ref() else {
                        break;
                    };
                    let frame = next.trace.request.timeline_frame;
                    if frame > target {
                        break;
                    }
                    due = self.upcoming.take();
                    if worker.starts_shot(frame) {
                        break;
                    }
                }
                Ok(due)
            }
        }
    }

    pub fn receiving(&mut self, number: u32) {
        self.displayed = Some(number);
        if self.playing() {
            self.cursor = number;
        }
    }

    /// Start sound only after the picture for the requested start point reached present().
    pub fn after_present(&mut self, new_picture: bool) -> Result<()> {
        if new_picture
            && self.wants_play()
            && let Some(shown) = self.displayed
            && let Some(worker) = &self.transport
            && let Some(sound) = &mut self.sound
        {
            sound.allow_through(picture_limit(
                self.fps,
                shown,
                worker.shot_end(shown),
                self.duration,
            )?);
            if self.mode == Mode::Starting && shown == self.cursor {
                sound.play_from(export::sample_boundary(self.fps, shown)?);
                self.mode = Mode::Playing;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_deadlines_use_exact_frame_boundaries_and_limit_lead_at_a_cut() -> Result<()> {
        let ntsc = FrameRate(30_000, 1001);
        for n in 1..100 {
            let at = export::sample_boundary(ntsc, n)?;
            assert_eq!(due_frame(ntsc, at, 120), n);
            assert_eq!(due_frame(ntsc, at - 1, 120), n - 1);
        }
        let fps = FrameRate(30, 1);
        let at = |n| export::sample_boundary(fps, n);
        assert_eq!(picture_limit(fps, 10, 42, 240)?, at(10)? + MAX_AUDIO_LEAD);
        assert_eq!(picture_limit(fps, 139, 141, 240)?, at(142)?);
        assert!(picture_limit(fps, 141, 240, 240)? > at(142)?);
        assert_eq!(picture_limit(fps, 239, 240, 240)?, at(240)?);
        assert_eq!(due_frame(fps, at(240)?, 240), 239);
        Ok(())
    }
}
