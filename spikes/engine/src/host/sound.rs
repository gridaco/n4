//! The audition plays the same 48 kHz mix that offline export consumes. Only the
//! feeder seeks or reads the spool. Generation-tagged blocks keep old sound out
//! of the callback when the user resumes at a different frame.
use crate::{Project, export};
use anyhow::{Context, Result, ensure};
use sdl2::{
    AudioSubsystem,
    audio::{AudioCallback, AudioDevice, AudioSpecDesired},
};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread,
};

const CHANNELS: usize = 2;
const FRAME_BYTES: usize = CHANNELS * size_of::<f32>();
const BLOCK_FRAMES: usize = 1024;
const PEAKS: usize = 1536;
const MONITOR_GAIN: f32 = 0.25;

#[derive(Clone)]
pub struct PreparedSound {
    // The accepted mix may be opened independently by playback and export.
    mix: Option<Arc<tempfile::TempDir>>,
    frames: u64,
    pub peaks: Arc<[f32]>,
}

const MIX_FILENAME: &str = "timeline.f32le";

impl PreparedSound {
    pub(super) fn prepare(project: &Project, preparations: &AtomicU64) -> Result<Self> {
        let frames = export::sample_boundary(project.fps, project.duration()?)?;
        if project.audio.is_empty() {
            return Ok(Self {
                mix: None,
                frames,
                peaks: Arc::from([]),
            });
        }
        let byte_len = frames
            .checked_mul(FRAME_BYTES as u64)
            .context("timeline audio exceeds file offsets")?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(MIX_FILENAME);
        export::write_pcm(project, &path)?;
        let mut file = File::open(&path)?;
        ensure!(
            file.metadata()?.len() == byte_len,
            "mixed audio length changed"
        );
        let peaks = waveform(&mut file, frames)?;
        preparations.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            mix: Some(Arc::new(dir)),
            frames,
            peaks: peaks.into(),
        })
    }

    pub(crate) fn mix_path(&self) -> Option<PathBuf> {
        self.mix.as_ref().map(|mix| mix.path().join(MIX_FILENAME))
    }
}

fn waveform(file: &mut File, frames: u64) -> Result<Vec<f32>> {
    let bins = frames.min(PEAKS as u64);
    let mut peaks = Vec::with_capacity(bins as usize);
    let mut reader = BufReader::new(file);
    let mut bytes = [0u8; BLOCK_FRAMES * FRAME_BYTES];
    let mut pos = 0;
    for bin in 0..bins {
        let end = (u128::from(frames) * u128::from(bin + 1) / u128::from(bins)) as u64;
        let mut peak = 0.0f32;
        while pos < end {
            let n = (end - pos).min(BLOCK_FRAMES as u64) as usize;
            let data = &mut bytes[..n * FRAME_BYTES];
            reader.read_exact(data)?;
            for sample in data.as_chunks::<4>().0 {
                peak = peak.max(f32::from_le_bytes(*sample).abs());
            }
            pos += n as u64;
        }
        peaks.push(peak);
    }
    Ok(peaks)
}

#[derive(Clone, Copy)]
struct Cue {
    generation: u64,
    sample: u64,
}

struct Block {
    generation: u64,
    bytes: [u8; BLOCK_FRAMES * FRAME_BYTES],
    frames: usize,
}

#[derive(Default)]
struct Clock {
    sample: AtomicU64,
    // Exclusive PCM boundary granted by the latest presented picture.
    picture_limit: AtomicU64,
    held_samples: AtomicU64,
}

fn feed(mut file: File, total: u64, cues: Receiver<Cue>, output: SyncSender<Block>) -> Result<()> {
    while let Ok(mut cue) = cues.recv() {
        loop {
            // The receiver can be idle during a pause. Discard intermediate seeks.
            for next in cues.try_iter() {
                cue = next;
            }
            file.seek(SeekFrom::Start(cue.sample * FRAME_BYTES as u64))?;
            let mut cursor = cue.sample;
            let next = loop {
                if let Ok(next) = cues.try_recv() {
                    break Some(next);
                }
                if cursor >= total {
                    break None;
                }
                let frames = (total - cursor).min(BLOCK_FRAMES as u64) as usize;
                let mut block = Block {
                    generation: cue.generation,
                    bytes: [0; BLOCK_FRAMES * FRAME_BYTES],
                    frames,
                };
                file.read_exact(&mut block.bytes[..frames * FRAME_BYTES])?;
                if output.send(block).is_err() {
                    return Ok(());
                }
                cursor += frames as u64;
            };
            match next {
                Some(next) => cue = next,
                None => break,
            }
        }
    }
    Ok(())
}

struct Mixer {
    input: Receiver<Block>,
    current: Option<Block>,
    offset: usize,
    cue: Cue,
    total: u64,
    clock: Arc<Clock>,
    silent: bool,
}

impl Mixer {
    fn seek(&mut self, cue: Cue) {
        self.current = None;
        self.offset = 0;
        self.cue = cue;
        self.clock.sample.store(cue.sample, Ordering::Release);
    }
}

impl AudioCallback for Mixer {
    type Channel = f32;
    fn callback(&mut self, out: &mut [f32]) {
        // SDL requests a new block after the previous one. Publish the start of
        // this block, including the exclusive end after a short final callback.
        self.clock.sample.store(self.cue.sample, Ordering::Release);
        out.fill(0.);
        let limit = self
            .clock
            .picture_limit
            .load(Ordering::Acquire)
            .min(self.total);
        let requested = out.len() / CHANNELS;
        let available = limit.saturating_sub(self.cue.sample).min(requested as u64) as usize;
        let mut written = if self.silent { available } else { 0 };
        while written < available {
            if self.current.is_none() {
                let Ok(block) = self.input.try_recv() else {
                    break;
                };
                if block.generation != self.cue.generation {
                    continue;
                }
                self.current = Some(block);
                self.offset = 0;
            }
            let block = self.current.as_ref().unwrap();
            let count = (block.frames - self.offset).min(available - written);
            let data = &block.bytes[self.offset * FRAME_BYTES..(self.offset + count) * FRAME_BYTES];
            for (dst, src) in out[written * CHANNELS..(written + count) * CHANNELS]
                .iter_mut()
                .zip(data.as_chunks::<4>().0.iter())
            {
                *dst = f32::from_le_bytes(*src) * MONITOR_GAIN;
            }
            self.offset += count;
            written += count;
            if self.offset == block.frames {
                self.current = None;
            }
        }
        self.cue.sample += written as u64;
        if self.cue.sample == limit && limit < self.total && written < requested {
            // Silence already fills the tail. The next callback resumes the
            // current PCM block at its first unplayed sample when the limit moves.
            self.clock
                .held_samples
                .fetch_add((requested - written) as u64, Ordering::Relaxed);
        }
    }
}

struct Feeder {
    cues: Option<Sender<Cue>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.cues.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct Sound {
    // Close SDL and its block receiver before joining a potentially full feeder.
    device: AudioDevice<Mixer>,
    feeder: Feeder,
    clock: Arc<Clock>,
    failed: Arc<AtomicBool>,
    generation: u64,
    total: u64,
}

impl Sound {
    pub fn open(audio: &AudioSubsystem, prepared: &PreparedSound) -> Result<Self> {
        let total = prepared.frames;
        let file = prepared.mix_path().map(File::open).transpose()?;
        let silent = file.is_none();
        let (send_blocks, input) = mpsc::sync_channel(4);
        let clock = Arc::new(Clock::default());
        let start = Cue {
            generation: 0,
            sample: 0,
        };
        let device = audio
            .open_playback(
                None,
                &AudioSpecDesired {
                    freq: Some(export::SAMPLE_RATE as i32),
                    channels: Some(CHANNELS as u8),
                    samples: Some(256),
                },
                |_| Mixer {
                    input,
                    current: None,
                    offset: 0,
                    cue: start,
                    total,
                    clock: Arc::clone(&clock),
                    silent,
                },
            )
            .map_err(anyhow::Error::msg)
            .context("open the default audio output")?;
        ensure!(
            device.spec().freq == export::SAMPLE_RATE as i32
                && device.spec().channels == CHANNELS as u8,
            "audio device did not provide 48 kHz stereo"
        );
        let failed = Arc::new(AtomicBool::new(false));
        let mut feeder = Feeder {
            cues: None,
            worker: None,
        };
        if let Some(file) = file {
            let (cues, commands) = mpsc::channel();
            let signal = Arc::clone(&failed);
            feeder.worker = Some(
                thread::Builder::new()
                    .name("n4-player-audio".into())
                    .spawn(move || {
                        if let Err(error) = feed(file, total, commands, send_blocks) {
                            eprintln!("N4 audio spool: {error:#}");
                            signal.store(true, Ordering::Release);
                        }
                    })?,
            );
            feeder.cues = Some(cues);
        }
        Ok(Self {
            device,
            feeder,
            clock,
            failed,
            generation: 0,
            total,
        })
    }

    pub fn pause(&mut self) {
        self.device.pause();
    }

    pub fn play_from(&mut self, sample: u64) {
        self.device.pause();
        self.generation = self.generation.wrapping_add(1);
        let cue = Cue {
            generation: self.generation,
            sample: sample.min(self.total),
        };
        self.device.lock().seek(cue);
        if let Some(sender) = &self.feeder.cues {
            let _ = sender.send(cue);
        }
        self.device.resume();
    }

    pub fn current_sample(&self) -> u64 {
        self.clock.sample.load(Ordering::Acquire)
    }

    pub fn allow_through(&self, end: u64) {
        self.clock.picture_limit.store(end, Ordering::Release);
    }

    pub fn picture_hold_samples(&self) -> u64 {
        self.clock.held_samples.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> Result<()> {
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "could not read the prepared audio mix"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn seeks_skip_stale_blocks_and_hold_until_samples_arrive() -> Result<()> {
        let (tx, input) = mpsc::sync_channel(4);
        let clock = Arc::new(Clock::default());
        clock.picture_limit.store(6, Ordering::Relaxed);
        let mut mixer = Mixer {
            input,
            current: None,
            offset: 0,
            total: 6,
            cue: Cue {
                generation: 0,
                sample: 0,
            },
            clock: Arc::clone(&clock),
            silent: false,
        };
        let mut stale = Block {
            generation: 0,
            frames: 4,
            bytes: [0; BLOCK_FRAMES * FRAME_BYTES],
        };
        stale.bytes[..4].copy_from_slice(&0.75f32.to_le_bytes());
        tx.send(stale)?;
        mixer.seek(Cue {
            generation: 1,
            sample: 4,
        });
        let mut out = [1.; 8];
        mixer.callback(&mut out);
        assert_eq!(out, [0.; 8]);
        assert_eq!(clock.sample.load(Ordering::Acquire), 4);
        let mut last = Block {
            generation: 1,
            frames: 2,
            bytes: [0; BLOCK_FRAMES * FRAME_BYTES],
        };
        for (dst, v) in last.bytes[..16]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([0.4f32, -0.8, 0.2, -0.6])
        {
            *dst = v.to_le_bytes();
        }
        tx.send(last)?;
        mixer.callback(&mut out);
        assert_eq!(
            out,
            [
                0.4 * MONITOR_GAIN,
                -0.8 * MONITOR_GAIN,
                0.2 * MONITOR_GAIN,
                -0.6 * MONITOR_GAIN,
                0.,
                0.,
                0.,
                0.
            ]
        );
        mixer.callback(&mut out);
        assert_eq!(clock.sample.load(Ordering::Acquire), 6);
        assert_eq!(out, [0.; 8]);
        Ok(())
    }

    #[test]
    fn picture_stall_emits_silence_then_resumes_at_first_unheard_pcm_sample() -> Result<()> {
        let (send, input) = mpsc::sync_channel(1);
        let clock = Arc::new(Clock::default());
        clock.picture_limit.store(3, Ordering::Relaxed);
        let mut mixer = Mixer {
            input,
            current: None,
            offset: 0,
            cue: Cue {
                generation: 0,
                sample: 0,
            },
            total: 8,
            clock: Arc::clone(&clock),
            silent: false,
        };
        let mut block = Block {
            generation: 0,
            bytes: [0; BLOCK_FRAMES * FRAME_BYTES],
            frames: 8,
        };
        for (n, sample) in block.bytes[..8 * FRAME_BYTES]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            sample.copy_from_slice(&(n as f32 + 1.).to_le_bytes());
        }
        send.send(block)?;
        let mut played = Vec::new();
        for limit in [3, 3, 6, 8] {
            clock.picture_limit.store(limit, Ordering::Release);
            let mut out = [0.; 8];
            mixer.callback(&mut out);
            played.extend(out.into_iter().filter(|&v| v != 0.));
            assert!(mixer.cue.sample <= limit);
        }
        assert_eq!(
            played,
            (1..=16)
                .map(|n| n as f32 * MONITOR_GAIN)
                .collect::<Vec<_>>()
        );
        assert_eq!(mixer.cue.sample, 8);
        // One held sample, then four, then one. Padding after EOF is excluded.
        assert_eq!(clock.held_samples.load(Ordering::Relaxed), 6);
        Ok(())
    }

    #[test]
    fn inherited_audio_keeps_the_pcm_spool_and_peaks_alive_until_last_owner() -> Result<()> {
        let project = Project::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/av-cut/edit.json"),
        )?;
        let count = AtomicU64::new(0);
        let original = PreparedSound::prepare(&project, &count)?;
        let inherited = original.clone();
        let spool = inherited.mix_path().expect("fixture has a soundtrack");
        assert!(Arc::ptr_eq(&inherited.peaks, &original.peaks));
        assert_eq!(original.mix_path().as_ref(), Some(&spool));
        drop(original);
        assert_eq!(
            File::open(&spool)?.metadata()?.len(),
            inherited.frames * FRAME_BYTES as u64
        );
        assert_eq!(count.load(Ordering::Relaxed), 1);
        drop(inherited);
        assert!(!spool.exists());
        Ok(())
    }

    #[test]
    fn waveform_uses_both_channels_and_contains_the_final_sample() -> Result<()> {
        let mut file = tempfile::tempfile()?;
        for _ in 0..3000 {
            file.write_all(&0.0_f32.to_le_bytes())?;
            file.write_all(&0.0_f32.to_le_bytes())?;
        }
        file.write_all(&0.0_f32.to_le_bytes())?;
        file.write_all(&(-0.75_f32).to_le_bytes())?;
        file.rewind()?;
        let peaks = waveform(&mut file, 3001)?;
        assert_eq!(peaks.len(), PEAKS);
        assert!(peaks[..PEAKS - 1].iter().all(|&v| v == 0.));
        assert_eq!(peaks[PEAKS - 1], 0.75);
        Ok(())
    }
}
