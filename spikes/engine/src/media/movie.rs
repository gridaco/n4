//! FFmpeg stream admission, indexed movie timing and decoding through the CLI or native library.
use super::PresentationIndex;
use crate::FrameRate;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(not(feature = "native-decode"))]
use std::{
    io::Read,
    process::{Child, ChildStdout, Stdio},
};

pub(super) struct Movie {
    pub(super) path: PathBuf,
    pub(super) filter: String,
    pub(super) decode_threads: u8,
    #[cfg(feature = "native-decode")]
    pub(super) keyframes: Vec<u32>,
}

#[cfg(not(feature = "native-decode"))]
pub(super) struct MovieCursor {
    child: Child,
    stdout: ChildStdout,
    next: u32,
}

#[cfg(not(feature = "native-decode"))]
impl Drop for MovieCursor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Movie {
    pub(super) fn open(
        path: &Path,
        width: u32,
        height: u32,
        fps: FrameRate,
    ) -> Result<(Self, PresentationIndex)> {
        let path = path
            .canonicalize()
            .with_context(|| format!("open {}", path.display()))?;
        let output = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_streams", "-show_frames",
                "-show_entries",
                "stream=width,height,time_base,avg_frame_rate,start_pts,duration_ts,sample_aspect_ratio,color_range,color_space,color_transfer,color_primaries,pix_fmt:stream_side_data=rotation:frame=pts,key_frame,duration,width,height,color_range,color_space,color_transfer,color_primaries,pix_fmt",
                "-of", "json", "-i"])
            .arg(&path)
            .output()
            .context("start ffprobe to index the video")?;
        ensure!(
            output.status.success(),
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let probe: Probe =
            serde_json::from_slice(&output.stdout).context("read ffprobe frame index")?;
        let stream = probe.streams.first().context("input has no video stream")?;
        super::check_aspect_ratio([stream.width, stream.height], [width, height])?;
        ensure!(
            matches!(
                stream.sample_aspect_ratio.as_deref(),
                None | Some("N/A" | "1:1")
            ),
            "non-square pixels require display geometry mapping"
        );
        ensure!(
            !stream
                .side_data_list
                .iter()
                .any(|side| side.rotation.is_some_and(|r| r % 360 != 0)),
            "rotated video requires display geometry mapping"
        );
        let transfer = stream.color.transfer()?;
        ensure!(
            probe.frames.iter().all(|frame| frame.color == stream.color),
            "source color profile or pixel format changes within stream"
        );
        ensure!(
            probe
                .frames
                .iter()
                .all(|f| (f.width, f.height) == (stream.width, stream.height)),
            "source geometry changes within stream"
        );
        let index = PresentationIndex::from_probe(&probe.frames, stream, fps)?;
        let same_size = (stream.width, stream.height) == (width, height);
        let resize = format!("scale={width}:{height}:flags=lanczos");
        let filter = match transfer {
            SourceTransfer::Srgb if same_size => "format=rgba".to_owned(),
            SourceTransfer::Srgb => format!("format=rgba,{resize}"),
            // The transfer already is sRGB. Decode the limited-range 601 matrix
            // directly to full-range RGB; primaries are 709/sRGB throughout.
            SourceTransfer::Srgb601Limited => {
                format!("{resize}:in_color_matrix=bt601:in_range=tv:out_range=pc,format=rgba")
            }
            SourceTransfer::Bt709Limited if same_size => BT709_TO_SRGB.to_owned(),
            // Reconstruct full chroma at target size before transfer conversion.
            SourceTransfer::Bt709Limited if width < stream.width => {
                format!("{resize},format=yuv444p12,{BT709_TO_SRGB}")
            }
            SourceTransfer::Bt709Limited => format!("{BT709_TO_SRGB},{resize}"),
        };
        Ok((
            Self {
                path,
                #[cfg(feature = "native-decode")]
                keyframes: probe
                    .frames
                    .iter()
                    .enumerate()
                    .filter(|(_, frame)| frame.key_frame != 0)
                    .map(|(i, _)| i as u32)
                    .collect(),
                filter,
                // UHD random access benefits from parallel decoding of long GOP preroll.
                // Keep smaller sources on the previously measured four-thread path.
                decode_threads: if u64::from(stream.width) * u64::from(stream.height) >= 3840 * 2160
                {
                    8
                } else {
                    4
                },
            },
            index,
        ))
    }

    #[cfg(not(feature = "native-decode"))]
    pub(super) fn decode(
        &self,
        index: &PresentationIndex,
        rgba_len: usize,
        ordinal: u32,
        _preroll: usize,
        active: &mut Option<MovieCursor>,
    ) -> Result<Vec<u8>> {
        let pts = *index
            .pts
            .get(ordinal as usize)
            .context("source ordinal out of bounds")?;
        if !active.as_ref().is_some_and(|cursor| {
            // A short forward scrub can consume an in-flight decode. A seek
            // farther away should skip codec preroll via the demuxer's index.
            ordinal.checked_sub(cursor.next).is_some_and(|gap| gap <= 2)
        }) {
            // Drop the old decoder before opening another. FFmpeg uses the
            // demuxer's keyframe index, decodes preroll, then trims at the exact
            // source PTS. Only delivered frames cross the raw RGBA pipe.
            *active = None;
            let mut cmd = Command::new("ffmpeg");
            cmd.args([
                "-nostdin",
                "-v",
                "error",
                "-xerror",
                "-copyts",
                "-noautorotate",
                "-filter_threads",
                "3",
                "-threads:v",
            ])
            .arg(self.decode_threads.to_string());
            let (num, den) = index.time_base;
            // Seek a microsecond early so conversion to FFmpeg's input time unit
            // cannot round past the target. Keep the first/negative-origin case
            // on the default demuxer start position.
            let micros = i128::from(pts) * num * 1_000_000 / den - 1;
            if ordinal > 0 && micros > 0 {
                cmd.args(["-noaccurate_seek", "-seek_timestamp", "1", "-ss"])
                    .arg(format!("{micros}us"));
            }
            let mut child = cmd
                .arg("-i")
                .arg(&self.path)
                .args(["-map", "0:v:0", "-vf"])
                .arg(format!("trim=start_pts={pts},{}", self.filter))
                .args([
                    "-fps_mode",
                    "passthrough",
                    "-threads:v",
                    "1",
                    "-pix_fmt",
                    "rgba",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ])
                .stdout(Stdio::piped())
                .spawn()
                .context("start ffmpeg video decoder")?;
            let stdout = child.stdout.take().context("open ffmpeg video output")?;
            *active = Some(MovieCursor {
                child,
                stdout,
                next: ordinal,
            });
        }
        let cursor = active.as_mut().context("missing decode cursor")?;
        let mut rgba = vec![0; rgba_len];
        while cursor.next <= ordinal {
            if let Err(e) = cursor.stdout.read_exact(&mut rgba) {
                *active = None;
                return Err(e).with_context(|| {
                    format!(
                        "read {} at display frame {ordinal} (PTS {pts})",
                        self.path.display()
                    )
                });
            }
            cursor.next += 1;
        }
        Ok(rgba)
    }
}

impl PresentationIndex {
    fn from_probe(frames: &[Frame], stream: &Stream, fps: FrameRate) -> Result<Self> {
        let pts: Vec<_> = frames.iter().map(|f| f.pts).collect();
        let &first = pts.first().context("video has no displayed frames")?;
        let &last = pts.last().context("video has no displayed frames")?;
        ensure!(
            pts.windows(2).all(|pair| pair[0] < pair[1]),
            "non-increasing presentation times"
        );
        ensure!(pts.len() <= u32::MAX as usize, "too many source frames");
        let (num, den) = positive_ratio(&stream.time_base)?;
        let relative = |t: i64| i128::from(t) - i128::from(first);
        let declared_end = frames
            .last()
            .and_then(|f| f.duration)
            .filter(|&d| d > 0)
            .map(|d| relative(last) + i128::from(d))
            .or_else(|| Some(relative(stream.start_pts?) + i128::from(stream.duration_ts?)))
            .filter(|&e| e > relative(last));
        let cfr = positive_ratio(&stream.avg_frame_rate)
            .ok()
            .filter(|&(rn, rd)| {
                let scale = num * rn;
                let near_grid =
                    |t: i128, i: usize| 2 * (t * scale - i as i128 * den * rd).abs() <= scale;
                pts.iter()
                    .enumerate()
                    .all(|(i, &p)| near_grid(relative(p), i))
                    // The container may round both the final PTS and its
                    // duration to ticks. Their sum can be one tick off the grid.
                    && declared_end.is_none_or(|end|
                        (end * scale - pts.len() as i128 * den * rd).abs() <= scale)
            });
        let (duration_num, duration_den) = match cfr {
            Some((rn, rd)) => (pts.len() as i128 * rd, rn),
            None => (
                declared_end.context("VFR movie needs a final frame duration")? * num,
                den,
            ),
        };
        let ticks = super::project_tick_count((duration_num, duration_den), fps)?;
        Ok(Self {
            pts: pts.into(),
            time_base: (num, den),
            cfr,
            fps,
            ticks,
            sample_end: declared_end.map_or((duration_num, duration_den), |end| (end * num, den)),
        })
    }
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct Stream {
    width: u32,
    height: u32,
    time_base: String,
    avg_frame_rate: String,
    start_pts: Option<i64>,
    duration_ts: Option<i64>,
    sample_aspect_ratio: Option<String>,
    #[serde(flatten)]
    color: Color,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<i32>,
}

#[derive(Deserialize)]
struct Frame {
    #[cfg(feature = "native-decode")]
    key_frame: u8,
    #[serde(flatten)]
    color: Color,
    pts: i64,
    duration: Option<i64>,
    width: u32,
    height: u32,
}

/// `colorspace` applies the source EOTF and destination OETF. Keep 12-bit 4:4:4
/// through this conversion; then use the declared matrix and range to obtain RGB.
const BT709_TO_SRGB: &str = "colorspace=iall=bt709:irange=tv:all=bt709:trc=srgb:range=pc:format=yuv444p12,scale=in_color_matrix=bt709:in_range=full:out_range=full,format=rgba";

#[derive(Clone, Copy, PartialEq)]
enum SourceTransfer {
    Srgb,
    Srgb601Limited,
    Bt709Limited,
}

#[derive(Default, Debug, PartialEq, Deserialize)]
struct Color {
    pix_fmt: String,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
}

impl Color {
    fn transfer(&self) -> Result<SourceTransfer> {
        let space = self.color_space.as_deref();
        let trc = self.color_transfer.as_deref();
        let primaries = self.color_primaries.as_deref();
        let range = self.color_range.as_deref();
        match (self.pix_fmt.as_str(), space, trc, primaries, range) {
            (
                "bgr0" | "rgb0" | "rgb24" | "bgr24",
                Some("gbr") | None,
                Some("iec61966-2-1") | None,
                Some("bt709") | None,
                Some("pc") | None,
            ) => Ok(SourceTransfer::Srgb),
            (
                "yuv420p" | "yuv422p" | "yuv444p",
                Some("smpte170m"),
                Some("iec61966-2-1"),
                Some("bt709"),
                Some("tv"),
            ) => Ok(SourceTransfer::Srgb601Limited),
            (
                "yuv420p" | "yuv422p" | "yuv444p",
                Some("bt709"),
                Some("bt709"),
                Some("bt709"),
                Some("tv"),
            ) => Ok(SourceTransfer::Bt709Limited),
            _ => bail!("unsupported source color profile: {self:?}"),
        }
    }
}

fn positive_ratio(s: &str) -> Result<(i128, i128)> {
    let (a, b) = s.split_once('/').context("missing rational denominator")?;
    let (a, b) = (a.parse::<i32>()?, b.parse::<i32>()?);
    ensure!(a > 0 && b > 0, "invalid rate or time base: {s}");
    Ok((i128::from(a), i128::from(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{IndexedVideo, VideoReader};
    use std::io::Write;
    use std::process::Stdio;

    #[test]
    fn tagged_srgb_with_a_limited_601_matrix_keeps_its_transfer() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let raw = dir.path().join("patches.yuv");
        let movie = dir.path().join("patches.mp4");
        // Planar Y'CbCr: black, sRGB midgray, white, and a red primary.
        let mut planes = Vec::new();
        for codes in [
            [16, 126, 235, 81],
            [128, 128, 128, 90],
            [128, 128, 128, 240],
        ] {
            let row: Vec<u8> = codes.into_iter().flat_map(|v| [v; 16]).collect();
            planes.extend(row.repeat(16));
        }
        std::fs::write(&raw, planes)?;
        let output = Command::new("ffmpeg")
            .args([
                "-v", "error", "-f", "rawvideo", "-pix_fmt", "yuv444p", "-s", "64x16", "-r", "1",
                "-i",
            ])
            .arg(&raw)
            .args([
                "-c:v",
                "libx264",
                "-crf",
                "0",
                "-preset",
                "ultrafast",
                "-x264-params",
                "colorprim=bt709:transfer=iec61966-2-1:colormatrix=smpte170m",
                "-color_range",
                "tv",
            ])
            .arg(&movie)
            .output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for (w, h) in [(64, 16), (32, 8)] {
            let source = IndexedVideo::open(&movie, w, h, FrameRate(1, 1))?;
            let (rgba, _) = VideoReader::default().read(&source, 0)?;
            for (i, rgb) in [[0, 0, 0], [128, 128, 128], [255, 255, 255], [254, 0, 0]]
                .iter()
                .enumerate()
            {
                let offset = 4 * ((w * h / 2 + w / 8) as usize + i * w as usize / 4);
                assert!(
                    rgba.rgba[offset..offset + 3]
                        .iter()
                        .zip(rgb)
                        .all(|(a, b)| a.abs_diff(*b) <= 2),
                    "at {w}×{h}, patch {i}: {:?}",
                    &rgba.rgba[offset..offset + 3]
                );
            }
        }
        Ok(())
    }

    #[test]
    fn limited_rec709_patches_are_converted_to_full_range_srgb() -> Result<()> {
        // Known 8-bit limited Y'CbCr patches: black, neutral midtone, white and
        // a saturated warm patch. Reference bytes below are from the BT.709
        // Y'CbCr matrix, inverse transfer and forward sRGB transfer, rounded to u8.
        let mut raw = vec![];
        for plane_row in [
            [16, 126, 235, 120],
            [128, 128, 128, 90],
            [128, 128, 128, 160],
        ] {
            raw.extend(plane_row.repeat(4));
        }
        let mut process = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "rawvideo",
                "-pixel_format",
                "yuv444p",
                "-video_size",
                "4x4",
                "-i",
                "pipe:0",
                "-vf",
                BT709_TO_SRGB,
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        process
            .stdin
            .take()
            .context("open ffmpeg test input")?
            .write_all(&raw)?;
        let out = process.wait_with_output()?;
        ensure!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout.len(), 4 * 4 * 4);
        for row in out.stdout.as_chunks::<16>().0 {
            let expected = [[0u8; 3], [140; 3], [255; 3], [186, 125, 57]];
            for (pixel, channels) in row.as_chunks::<4>().0.iter().zip(expected) {
                assert_eq!(pixel[3], 255);
                for (value, reference) in pixel[..3].iter().zip(channels) {
                    assert!(value.abs_diff(reference) <= 1, "{pixel:?} vs {channels:?}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn seek_vfr_b_frames_at_a_nonzero_origin_after_downscaling() -> Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/numbered-vfr.mp4");
        let video = IndexedVideo::open(&path, 320, 180, FrameRate(30, 1))?;
        let mut reader = VideoReader::with_limits(2 * video.rgba_len, 0);
        assert_eq!(video.frame_count(), 102);
        // The immutable barcode is source ordinal n. The fixture schedules it
        // at tick n+floor(n/7). Seek on either side of long holds and GOP cuts.
        for (tick, expected) in [
            (0, 0),
            (6, 6),
            (7, 6),
            (8, 7),
            (26, 23),
            (27, 24),
            (28, 25),
            (100, 88),
            (101, 89),
            (27, 24),
            (1, 1),
        ] {
            let actual = video.resolve(tick)?;
            assert_eq!(actual, expected);
            let (rgba, _hit) = reader.read(&video, actual)?;
            let barcode = (0..7).fold(0, |id, bit| {
                let red = rgba.rgba[4 * (172 * 320 + 8 + 8 * bit)];
                id | (u32::from(red > 128) << bit)
            });
            assert_eq!(barcode, expected, "source tick {tick}");
        }
        assert!(video.resolve(102).is_err());
        assert!(reader.read(&video, 1)?.1);
        assert!(!reader.read(&video, 2)?.1);
        assert!(reader.read(&video, 1)?.1);
        assert!(!reader.read(&video, 3)?.1);
        assert!(
            !reader.read(&video, 2)?.1,
            "least recent frame should be evicted"
        );
        assert_eq!(reader.bytes, 2 * video.rgba_len);
        let thumbnail = IndexedVideo::open(&path, 160, 90, FrameRate(30, 1))?;
        let (other_size, hit) = reader.read(&thumbnail, 2)?;
        assert!(
            !hit,
            "the same path opened at a different size needs different pixels"
        );
        assert_eq!(other_size.rgba.len(), 160 * 90 * 4);
        reader.retain_sources(|_| false);
        assert!(reader.cursors.is_empty());
        assert_eq!(reader.bytes, 0);
        Ok(())
    }

    /// Local media-only measurements. Excludes GPU, device presentation and
    /// audio. The OS file cache is uncontrolled. Supply a 16:9 SDR original.
    #[test]
    #[ignore = "manual 1080p decoder measurement with an external sample"]
    fn measure_media_requests() -> Result<()> {
        use std::time::Instant;
        let file = std::env::var_os("N4_MEDIA_SAMPLE").context("set N4_MEDIA_SAMPLE")?;
        let start = Instant::now();
        let video = IndexedVideo::open(Path::new(&file), 1920, 1080, FrameRate(30, 1))?;
        println!("index_ms={:.2}", start.elapsed().as_secs_f64() * 1000.);
        let len = u32::try_from(video.index.pts.len())?;
        ensure!(len > 120, "sample is too short");
        let mut targets = Vec::new();
        // Distinct reproducible locations spread over the source duration.
        for i in 0..30u32 {
            targets.push(u32::try_from(
                (u64::from(len - 1) * u64::from((i * 17) % 30)) / 30,
            )?);
        }
        let mut reader = VideoReader::default();
        for phase in ["first_visit", "revisit"] {
            let mut latencies = Vec::new();
            let mut hits = 0;
            for &ordinal in &targets {
                let start = Instant::now();
                let (pixels, hit) = reader.read(&video, ordinal)?;
                hits += u32::from(hit);
                std::hint::black_box(&pixels);
                latencies.push(start.elapsed().as_micros());
            }
            assert_eq!(
                hits,
                if phase == "revisit" {
                    targets.len() as u32
                } else {
                    0
                }
            );
            println!("{phase}_rgba_hits={hits}");
            println!("{phase}_us={latencies:?}");
            latencies.sort_unstable();
            println!("{phase}_p95_ms={:.2}", latencies[28] as f64 / 1000.);
        }
        let start = Instant::now();
        for ordinal in 0..120 {
            std::hint::black_box(reader.read(&video, ordinal)?);
        }
        println!(
            "sequential_120_fps={:.2}",
            120. / start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    #[test]
    fn presentation_ticks_hold_until_next_pts_and_exclude_the_end() -> Result<()> {
        let stream = Stream {
            width: 2,
            height: 2,
            avg_frame_rate: "30/1".into(),
            time_base: "1/1000".into(),
            start_pts: Some(-100),
            duration_ts: None,
            sample_aspect_ratio: None,
            color: Color::default(),
            side_data_list: vec![],
        };
        let frames = |times: &[i64], duration: i64| -> Vec<Frame> {
            times
                .iter()
                .map(|&pts| Frame {
                    #[cfg(feature = "native-decode")]
                    key_frame: 0,
                    color: Color::default(),
                    pts,
                    duration: Some(duration),
                    width: 2,
                    height: 2,
                })
                .collect()
        };
        let grid = PresentationIndex::from_probe(
            &frames(&[-100, -67, -33, 0], 33),
            &stream,
            FrameRate(30, 1),
        )?;
        assert_eq!(grid.cfr, Some((30, 1)));
        assert_eq!(grid.resolve(2)?, 2);
        assert!(grid.resolve(4).is_err());
        let variable = PresentationIndex::from_probe(
            &frames(&[-100, -80, -29, 3], 10),
            &stream,
            FrameRate(30, 1),
        )?;
        assert!(variable.cfr.is_none());
        assert_eq!(variable.ticks, 4);
        assert_eq!(
            (
                variable.resolve(0)?,
                variable.resolve(1)?,
                variable.resolve(2)?,
                variable.resolve(3)?
            ),
            (0, 1, 1, 2)
        );
        assert!(variable.resolve(4).is_err());
        Ok(())
    }

    #[test]
    fn proxy_requires_identical_physical_and_resolved_boundaries_through_eof() -> Result<()> {
        let index = |times: &[i64], last: i64, time_base: &str, rate: &str| {
            let stream = Stream {
                width: 2,
                height: 2,
                avg_frame_rate: rate.into(),
                time_base: time_base.into(),
                start_pts: None,
                duration_ts: None,
                sample_aspect_ratio: None,
                color: Color::default(),
                side_data_list: vec![],
            };
            let frames = times
                .iter()
                .map(|&pts| Frame {
                    #[cfg(feature = "native-decode")]
                    key_frame: 0,
                    color: Color::default(),
                    pts,
                    duration: Some(last),
                    width: 2,
                    height: 2,
                })
                .collect::<Vec<_>>();
            PresentationIndex::from_probe(&frames, &stream, FrameRate(30, 1))
        };
        let vfr = index(&[-100, -80, -29, 3], 10, "1/1000", "30/1")?;
        let equivalent = index(&[511, 571, 724, 820], 30, "1/3000", "0/0")?;
        vfr.check_proxy(&equivalent)?;
        let changed_pts = index(&[511, 572, 724, 820], 30, "1/3000", "0/0")?;
        assert!(vfr.check_proxy(&changed_pts).is_err());
        let longer_tail = index(&[511, 571, 724, 820], 31, "1/3000", "0/0")?;
        assert_eq!(longer_tail.ticks, vfr.ticks);
        assert!(vfr.check_proxy(&longer_tail).is_err());

        let cfr = index(&[0, 33, 67, 100], 33, "1/1000", "30/1")?;
        let changed_grid = index(&[0, 33, 67, 100], 33, "1/1000", "0/0")?;
        assert_ne!(cfr.resolve(2)?, changed_grid.resolve(2)?);
        assert!(cfr.check_proxy(&changed_grid).is_err());
        let changed_cfr_tail = index(&[0, 33, 67, 100], 34, "1/1000", "30/1")?;
        assert_eq!(changed_cfr_tail.cfr, cfr.cfr);
        assert!(cfr.check_proxy(&changed_cfr_tail).is_err());
        Ok(())
    }
}
