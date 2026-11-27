//! Offline SDR delivery. Audio is addressed in samples on a stereo 48 kHz bus.
//! Pictures come from the same Engine::render call used for full-quality inspection.
use crate::{Engine, FrameRate, Project};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::NamedTempFile;

pub const SAMPLE_RATE: u32 = 48_000;
const BYTES_PER_SAMPLE_FRAME: usize = 8;
const BLOCK: usize = 8192;

// Encode sRGB into 12-bit Y'CbCr 709 before converting the transfer function and subsampling.
// Merely tagging sRGB input as Rec. 709 changes the appearance of midtones.
const DELIVERY_COLOR: &str = "scale=out_color_matrix=bt709:out_range=full,format=yuv444p12,colorspace=iall=bt709:itrc=srgb:irange=pc:all=bt709:range=tv:format=yuv420p";

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioClip {
    pub source: PathBuf,
    pub start_sample: u64,
    pub source_in_sample: u64,
    pub samples: u64,
    #[serde(default = "unity")]
    pub gain: f32,
}

fn unity() -> f32 {
    1.0
}

/// Quantize an absolute frame boundary once. Subtract boundaries to get interval lengths.
pub fn sample_boundary(fps: FrameRate, frame: u32) -> Result<u64> {
    ensure!(fps.0 > 0 && fps.1 > 0, "invalid audio frame rate");
    let scaled = u128::from(frame) * u128::from(fps.1) * u128::from(SAMPLE_RATE);
    u64::try_from((scaled + u128::from(fps.0 / 2)) / u128::from(fps.0))
        .context("timeline exceeds the sample clock")
}

pub(crate) fn validate_audio(clips: &[AudioClip], fps: FrameRate, frames: u32) -> Result<()> {
    let end = sample_boundary(fps, frames)?;
    for clip in clips {
        ensure!(
            clip.gain.is_finite() && clip.gain >= 0.0,
            "invalid audio gain"
        );
        ensure!(clip.samples > 0, "audio clip has no samples");
        ensure!(
            clip.start_sample
                .checked_add(clip.samples)
                .is_some_and(|n| n <= end),
            "audio clip exceeds the project"
        );
        clip.source_in_sample
            .checked_add(clip.samples)
            .context("audio source range overflow")?;
    }
    Ok(())
}

fn check_source_range(clip: &AudioClip, available: u64) -> Result<()> {
    ensure!(
        clip.source_in_sample
            .checked_add(clip.samples)
            .is_some_and(|end| end <= available),
        "audio trim exceeds {} ({} available samples)",
        clip.source.display(),
        available
    );
    Ok(())
}

/// Counts use the same decoder and sample-rate conversion as export. The initial check
/// decodes each new source once. Subsequent trim and gain changes only read file metadata.
#[derive(Default)]
pub(crate) struct AudioSourceLengths {
    sources: HashMap<PathBuf, SourceLength>,
}

#[derive(Clone)]
struct SourceLength {
    bytes: u64,
    modified: std::time::SystemTime,
    samples: u64,
}

impl AudioSourceLengths {
    pub(crate) fn validate(&mut self, clips: &[AudioClip]) -> Result<()> {
        let mut next = HashMap::new();
        for clip in clips {
            let length = match next.entry(clip.source.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let file = std::fs::metadata(&clip.source)
                        .with_context(|| format!("inspect audio {}", clip.source.display()))?;
                    ensure!(
                        file.is_file(),
                        "expected audio file {}",
                        clip.source.display()
                    );
                    let bytes = file.len();
                    let modified = file.modified()?;
                    let known = self
                        .sources
                        .get(&clip.source)
                        .filter(|prior| prior.bytes == bytes && prior.modified == modified);
                    let length = match known {
                        Some(prior) => prior.clone(),
                        None => SourceLength {
                            bytes,
                            modified,
                            samples: PcmSource::open(&clip.source)?.samples,
                        },
                    };
                    e.insert(length)
                }
            };
            check_source_range(clip, length.samples)?;
        }
        self.sources = next;
        Ok(())
    }
}

struct PcmSource {
    file: File,
    samples: u64,
}

impl PcmSource {
    fn open(path: &Path) -> Result<Self> {
        let file = tempfile::tempfile()?;
        let result = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-xerror", "-i"])
            .arg(path)
            .args([
                "-map", "0:a:0", "-vn", "-ar", "48000", "-ac", "2", "-f", "f32le", "pipe:1",
            ])
            .stdout(file.try_clone()?)
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        ensure!(
            result.status.success(),
            "decode audio {}: {}",
            path.display(),
            String::from_utf8_lossy(&result.stderr)
        );
        let size = file.metadata()?.len();
        ensure!(
            size % BYTES_PER_SAMPLE_FRAME as u64 == 0,
            "incomplete decoded sample frame"
        );
        Ok(Self {
            file,
            samples: size / BYTES_PER_SAMPLE_FRAME as u64,
        })
    }
}

/// Write a new raw stereo f32le mix for a playback instrument or the offline encoder.
/// Source PCM is spooled once per asset, with bounded working memory for mixing and seeking.
pub fn write_pcm(project: &Project, out: &Path) -> Result<()> {
    project.validate()?;
    let mut sources = HashMap::new();
    for clip in &project.audio {
        let src = match sources.entry(&clip.source) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(PcmSource::open(&clip.source)?)
            }
        };
        check_source_range(clip, src.samples)?;
    }

    let end = sample_boundary(project.fps, project.duration()?)?;
    let mut staging = NamedTempFile::new_in(parent_dir(out))?;
    let mut bytes = vec![0u8; BLOCK * BYTES_PER_SAMPLE_FRAME];
    let mut mix = vec![0.0_f64; BLOCK * 2];
    let mut start = 0;
    while start < end {
        let count = (end - start).min(BLOCK as u64) as usize;
        mix[..count * 2].fill(0.0);
        let stop = start + count as u64;
        for clip in &project.audio {
            let first = start.max(clip.start_sample);
            let last = stop.min(clip.start_sample + clip.samples);
            if first >= last {
                continue;
            }
            let source = &mut sources
                .get_mut(&clip.source)
                .context("missing PCM source")?
                .file;
            let source_sample = clip.source_in_sample + (first - clip.start_sample);
            source.seek(SeekFrom::Start(
                source_sample * BYTES_PER_SAMPLE_FRAME as u64,
            ))?;
            let slice = &mut bytes[..(last - first) as usize * BYTES_PER_SAMPLE_FRAME];
            source.read_exact(slice)?;
            let offset = (first - start) as usize * 2;
            for (i, sample) in slice.as_chunks::<4>().0.iter().enumerate() {
                mix[offset + i] += f64::from(f32::from_le_bytes(*sample)) * f64::from(clip.gain);
            }
        }
        let encoded = &mut bytes[..count * BYTES_PER_SAMPLE_FRAME];
        for (dst, value) in encoded.as_chunks_mut::<4>().0.iter_mut().zip(&mix) {
            *dst = (*value as f32).clamp(-1.0, 1.0).to_le_bytes();
        }
        staging.write_all(encoded)?;
        start = stop;
    }
    staging
        .persist_noclobber(out)
        .with_context(|| format!("publish PCM {}", out.display()))?;
    Ok(())
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

#[derive(Debug, Serialize)]
pub struct ExportReport {
    pub frames: u32,
    pub audio_sample_frames: u64,
    pub audio_sample_rate: u32,
    /// SHA-256 over all input RGBA frames fed to the encoder before chroma/transfer conversion.
    pub rgba_sha256: String,
}

pub fn write_mp4(engine: &mut Engine, out: &Path) -> Result<ExportReport> {
    write_mp4_with_progress(engine, out, None, |_| Ok(()))
}

/// Use the accepted mix when the caller already prepared this revision's audio.
/// The checkpoint can stop the encode between pictures or before publication.
pub(crate) fn write_mp4_with_progress(
    engine: &mut Engine,
    out: &Path,
    prepared_audio: Option<&Path>,
    mut checkpoint: impl FnMut(u32) -> Result<()>,
) -> Result<ExportReport> {
    let [width, height] = engine.project.size;
    ensure!(
        width % 2 == 0 && height % 2 == 0,
        "4:2:0 export needs even dimensions"
    );
    ensure!(!out.exists(), "output already exists: {}", out.display());
    let fps = engine.project.fps;
    let frames = engine.project.duration()?;
    let samples = sample_boundary(fps, frames)?;
    checkpoint(0)?;
    let work;
    let audio = if let Some(path) = prepared_audio {
        path.to_path_buf()
    } else {
        work = tempfile::tempdir()?;
        let path = work.path().join("timeline.f32le");
        write_pcm(&engine.project, &path)?;
        path
    };
    checkpoint(0)?;
    let movie = NamedTempFile::new_in(parent_dir(out))?;
    let mut errors = tempfile::tempfile()?;
    let mut child = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-y",
            "-v",
            "error",
            "-xerror",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            &format!("{}/{}", fps.0, fps.1),
            "-i",
            "pipe:0",
            "-f",
            "f32le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-i",
        ])
        .arg(&audio)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-vf",
            DELIVERY_COLOR,
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "17",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_primaries",
            "bt709",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(movie.path())
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-f",
            "streamhash",
            "-hash",
            "sha256",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(errors.try_clone()?)
        .spawn()
        .context("start FFmpeg delivery encoder")?;
    let result = (|| -> Result<()> {
        let mut pipe = child.stdin.take().context("encoder input closed")?;
        for number in 0..frames {
            let frame = engine
                .render(number)
                .with_context(|| format!("render export frame {number}"))?;
            pipe.write_all(&frame.rgba)
                .context("feed compositor output to encoder")?;
            checkpoint(number + 1)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let output = child.wait_with_output()?;
    errors.rewind()?;
    let mut diagnostic = String::new();
    errors.read_to_string(&mut diagnostic)?;
    result.with_context(|| format!("encode frame stream: {diagnostic}"))?;
    ensure!(
        output.status.success(),
        "FFmpeg delivery failed: {diagnostic}"
    );
    let digest = std::str::from_utf8(&output.stdout)?
        .trim()
        .strip_prefix("0,v,SHA256=")
        .context("encoder did not return RGBA stream digest")?;
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid RGBA digest"
    );
    checkpoint(frames)?;
    movie
        .persist_noclobber(out)
        .with_context(|| format!("publish {}", out.display()))?;
    Ok(ExportReport {
        frames,
        audio_sample_frames: samples,
        audio_sample_rate: SAMPLE_RATE,
        rgba_sha256: digest.into(),
    })
}
