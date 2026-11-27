use crate::FrameRate;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::collections::VecDeque;
#[cfg(not(feature = "native-decode"))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(not(feature = "native-decode"))]
use std::process::{Child, ChildStdout, Stdio};
use std::sync::Arc;

/// Source ticks use the project frame rate. The presentation index maps them to
/// display ordinals independently of the codec packet order or stream PTS origin.
pub(crate) struct IndexedVideo {
    path: PathBuf,
    index: PresentationIndex,
    rgba_len: usize,
    filter: String,
    decode_threads: u8,
    key: Arc<PathBuf>,
}

/// A pipe is consumed in presentation order. Replacing or dropping the cursor
/// immediately reaps its process, including when the consumer stops mid-frame.
#[cfg(not(feature = "native-decode"))]
struct DecodeCursor {
    key: Arc<PathBuf>,
    child: Child,
    stdout: ChildStdout,
    next: u32,
}

#[cfg(feature = "native-decode")]
mod native;
#[cfg(feature = "native-decode")]
use native::DecodeCursor;

#[cfg(not(feature = "native-decode"))]
impl Drop for DecodeCursor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Bound codec sessions independently of the shared requested-frame and preroll byte budgets.
const MAX_DECODE_CURSORS: usize = 8;

/// Each independently decoded source/size profile in the current picture can
/// retain a cursor. Duplicate placements at the same size share the profile.
pub(crate) struct VideoReader {
    cursors: VecDeque<ReaderCursor>,
    active_profiles: Vec<Arc<PathBuf>>,
    recent: VecDeque<CachedFrame>,
    bytes: usize,
    limit: usize,
    #[cfg(feature = "native-decode")]
    preroll_limit: usize,
}

struct ReaderCursor {
    decoder: DecodeCursor,
    #[cfg(feature = "native-decode")]
    preroll_capacity: usize,
}

struct CachedFrame {
    source: Arc<PathBuf>,
    ordinal: u32,
    pixels: Arc<Vec<u8>>,
}

impl Default for VideoReader {
    fn default() -> Self {
        #[cfg(feature = "native-decode")]
        {
            Self::with_limits(256 * 1024 * 1024, 64 * 1024 * 1024)
        }
        #[cfg(not(feature = "native-decode"))]
        {
            Self::with_limits(256 * 1024 * 1024, 0)
        }
    }
}

impl VideoReader {
    pub(crate) fn with_limits(limit: usize, _preroll_limit: usize) -> Self {
        Self {
            cursors: VecDeque::new(),
            active_profiles: Vec::new(),
            recent: VecDeque::new(),
            bytes: 0,
            limit,
            #[cfg(feature = "native-decode")]
            preroll_limit: _preroll_limit.min(limit),
        }
    }

    pub(crate) fn begin_frame<'a>(&mut self, sources: impl IntoIterator<Item = &'a IndexedVideo>) {
        self.active_profiles.clear();
        for source in sources {
            if !self
                .active_profiles
                .iter()
                .any(|key| source.has_identity(key))
            {
                self.active_profiles.push(source.key.clone());
                if self.active_profiles.len() == MAX_DECODE_CURSORS {
                    break;
                }
            }
        }
        self.cursors.retain(|c| {
            self.active_profiles
                .iter()
                .any(|key| Arc::ptr_eq(key, &c.decoder.key))
        });
    }

    pub(crate) fn read(
        &mut self,
        source: &IndexedVideo,
        ordinal: u32,
    ) -> Result<(Arc<Vec<u8>>, bool)> {
        if let Some(i) = self
            .recent
            .iter()
            .position(|f| Arc::ptr_eq(&f.source, &source.key) && f.ordinal == ordinal)
        {
            let hit = self
                .recent
                .remove(i)
                .context("missing cached video frame")?;
            let pixels = Arc::clone(&hit.pixels);
            self.recent.push_back(hit);
            return Ok((pixels, true));
        }
        let mut active = self
            .cursors
            .iter()
            .position(|c| source.has_identity(&c.decoder.key))
            .and_then(|i| self.cursors.remove(i))
            .map(|c| c.decoder);
        let cursor_limit = self.active_profiles.len().max(1);
        if active.is_none() && self.cursors.len() >= cursor_limit {
            // Beyond the session cap, additional profiles share the last slot.
            // A cyclic scan must not evict every warm cursor on every picture.
            let pinned = &self.active_profiles[..cursor_limit - 1];
            let spare = self
                .cursors
                .iter()
                .position(|c| !pinned.iter().any(|key| Arc::ptr_eq(key, &c.decoder.key)));
            self.cursors.remove(spare.unwrap_or(0));
        }
        // A parked cursor may still own preroll from its previous read. Charge
        // that capacity until it is resized or dropped, including at clip boundaries.
        #[cfg(feature = "native-decode")]
        let parked: usize = self.cursors.iter().map(|c| c.preroll_capacity).sum();
        #[cfg(not(feature = "native-decode"))]
        let parked = 0;
        #[cfg(feature = "native-decode")]
        let preroll = if active.as_ref().is_some_and(|c| c.is_near(source, ordinal)) {
            (self.preroll_limit / cursor_limit).min(self.preroll_limit - parked)
        } else {
            0
        };
        #[cfg(not(feature = "native-decode"))]
        let preroll = 0;
        let rgba_budget = self.limit - parked - preroll;
        // Make room before handing buffer ownership to a decoder. Distant
        // jumps lend the entire budget back to the requested-frame LRU.
        let keep = rgba_budget
            .checked_sub(source.rgba_len)
            .unwrap_or(rgba_budget);
        while self.bytes > keep || self.recent.len() >= 64 {
            let evicted = self.recent.pop_front().context("missing LRU video frame")?;
            self.bytes -= evicted.pixels.len();
        }
        #[cfg(feature = "native-decode")]
        let pixels = Arc::new(source.decode(ordinal, preroll, &mut active)?);
        #[cfg(not(feature = "native-decode"))]
        let pixels = Arc::new(source.decode(ordinal, &mut active)?);
        if let Some(decoder) = active {
            self.cursors.push_back(ReaderCursor {
                decoder,
                #[cfg(feature = "native-decode")]
                preroll_capacity: preroll,
            });
        }
        if pixels.len() <= rgba_budget {
            self.bytes += pixels.len();
            self.recent.push_back(CachedFrame {
                source: source.key.clone(),
                ordinal,
                pixels: Arc::clone(&pixels),
            });
        }
        Ok((pixels, false))
    }

    pub(crate) fn retain_sources(&mut self, keep: impl Fn(&Arc<PathBuf>) -> bool) {
        self.recent.retain(|f| keep(&f.source));
        self.bytes = self.recent.iter().map(|f| f.pixels.len()).sum();
        self.cursors.retain(|c| keep(&c.decoder.key));
    }
}

impl IndexedVideo {
    pub(crate) fn has_identity(&self, key: &Arc<PathBuf>) -> bool {
        Arc::ptr_eq(&self.key, key)
    }

    pub(crate) fn open(path: &Path, width: u32, height: u32, fps: FrameRate) -> Result<Self> {
        ensure!(
            width > 0 && height > 0 && fps.0 > 0 && fps.1 > 0,
            "invalid video profile"
        );
        let rgba_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .context("video frame is too large")?;
        // An opened media profile owns its decode identity. Reopening a path for
        // a thumbnail size or a refreshed source cannot return old pixels.
        let key = Arc::new(path.to_path_buf());
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
        let h_times_src_w = u64::from(height) * u64::from(stream.width);
        let proportional = u64::from(width) * u64::from(stream.height);
        ensure!(
            h_times_src_w.abs_diff(proportional) <= u64::from(stream.width) / 2,
            "source and output aspect ratios differ; explicit framing is required"
        );
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
        Ok(Self {
            path,
            index,
            rgba_len,
            filter,
            // UHD random access benefits from parallel decoding of long GOP preroll.
            // Keep smaller sources on the previously measured four-thread path.
            decode_threads: if u64::from(stream.width) * u64::from(stream.height) >= 3840 * 2160 {
                8
            } else {
                4
            },
            key,
        })
    }

    /// Number of project-rate source ticks whose start lies inside the movie.
    pub(crate) fn frame_count(&self) -> u32 {
        self.index.ticks
    }

    pub(crate) fn resolve(&self, source_tick: u32) -> Result<u32> {
        self.index.resolve(source_tick)
    }

    /// A proxy may have a different codec, raster size, timescale or PTS origin.
    /// Its display ordinals must retain the exact source timing through EOF.
    pub(crate) fn check_proxy_timeline(&self, proxy: &Self) -> Result<()> {
        self.index.check_proxy(&proxy.index)
    }

    #[cfg(not(feature = "native-decode"))]
    fn decode(&self, ordinal: u32, active: &mut Option<DecodeCursor>) -> Result<Vec<u8>> {
        let pts = *self
            .index
            .pts
            .get(ordinal as usize)
            .context("source ordinal out of bounds")?;
        if !active.as_ref().is_some_and(|cursor| {
            // A short forward scrub can consume an in-flight decode. A seek
            // farther away should skip codec preroll via the demuxer's index.
            Arc::ptr_eq(&cursor.key, &self.key)
                && ordinal.checked_sub(cursor.next).is_some_and(|gap| gap <= 2)
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
            let (num, den) = self.index.time_base;
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
            *active = Some(DecodeCursor {
                key: self.key.clone(),
                child,
                stdout,
                next: ordinal,
            });
        }
        let cursor = active.as_mut().context("missing decode cursor")?;
        let mut rgba = vec![0; self.rgba_len];
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

/// Convert between source PTS and project-rate ticks using integer arithmetic.
/// Quantized CFR timestamps (e.g. 33/34 ms at 30 fps) use their verified native
/// frame grid. A variable frame is held until the next frame's actual PTS.
struct PresentationIndex {
    #[cfg(feature = "native-decode")]
    keyframes: Vec<u32>,
    pts: Vec<i64>,
    time_base: (i128, i128),
    cfr: Option<(i128, i128)>,
    fps: FrameRate,
    ticks: u32,
    /// End of the last displayed sample in seconds, relative to the first PTS.
    /// Use the verified CFR span only when no sample or stream duration exists.
    sample_end: (i128, i128),
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
        let a = duration_num * i128::from(fps.0);
        let b = duration_den * i128::from(fps.1);
        let ticks = u32::try_from((a + b - 1) / b).context("movie exceeds source tick domain")?;
        Ok(Self {
            #[cfg(feature = "native-decode")]
            keyframes: frames
                .iter()
                .enumerate()
                .filter(|(_, f)| f.key_frame != 0)
                .map(|(i, _)| i as u32)
                .collect(),
            pts,
            time_base: (num, den),
            cfr,
            fps,
            ticks,
            sample_end: declared_end.map_or((duration_num, duration_den), |end| (end * num, den)),
        })
    }

    fn check_proxy(&self, proxy: &Self) -> Result<()> {
        // Probe time-base terms fit i32, absolute PTS and durations fit i64,
        // and counts fit u32. Relative times and cross-products fit i128.
        let equal = |(a, b): (i128, i128), (c, d): (i128, i128)| a * d == c * b;
        ensure!(
            u64::from(self.fps.0) * u64::from(proxy.fps.1)
                == u64::from(proxy.fps.0) * u64::from(self.fps.1),
            "proxy project timebase differs"
        );
        ensure!(
            self.pts.len() == proxy.pts.len(),
            "proxy picture count differs"
        );
        ensure!(
            equal(self.sample_end, proxy.sample_end),
            "proxy final picture duration differs"
        );
        let boundary = |index: &Self, ordinal: usize| {
            let actual = if ordinal == index.pts.len() {
                index.sample_end
            } else {
                let relative = i128::from(index.pts[ordinal]) - i128::from(index.pts[0]);
                (relative * index.time_base.0, index.time_base.1)
            };
            let effective = index
                .cfr
                .map_or(actual, |(rn, rd)| (ordinal as i128 * rd, rn));
            (actual, effective)
        };
        for i in 0..=self.pts.len() {
            let (original, effective) = boundary(self, i);
            let (derived, resolved) = boundary(proxy, i);
            ensure!(
                equal(original, derived),
                "proxy PTS differs for picture {i}"
            );
            ensure!(
                equal(effective, resolved),
                "proxy resolves picture {i} on a different time grid"
            );
        }
        Ok(())
    }

    fn resolve(&self, tick: u32) -> Result<u32> {
        ensure!(
            tick < self.ticks,
            "source tick {tick} outside [0,{})",
            self.ticks
        );
        let t = i128::from(tick) * i128::from(self.fps.1);
        let rate = i128::from(self.fps.0);
        if let Some((rn, rd)) = self.cfr {
            return Ok(u32::try_from(t * rn / (rate * rd))?);
        }
        let (num, den) = self.time_base;
        let origin = i128::from(self.pts[0]);
        let after = self
            .pts
            .partition_point(|&p| (i128::from(p) - origin) * num * rate <= t * den);
        Ok(u32::try_from(after - 1)?)
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
                    rgba[offset..offset + 3]
                        .iter()
                        .zip(rgb)
                        .all(|(a, b)| a.abs_diff(*b) <= 2),
                    "at {w}×{h}, patch {i}: {:?}",
                    &rgba[offset..offset + 3]
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
                let red = rgba[4 * (172 * 320 + 8 + 8 * bit)];
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
        assert_eq!(other_size.len(), 160 * 90 * 4);
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
