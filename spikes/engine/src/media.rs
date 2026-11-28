//! Project-rate presentation lookup, decode profiles and bounded worker-local frame caches.
use crate::{FrameRate, MAX_SURFACE_DIMENSION};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

mod animation;
pub(crate) mod assets;
mod gif;
mod image_file;
mod movie;
#[cfg(feature = "native-decode")]
mod native;
pub(crate) mod still;
mod webp;
#[cfg(not(feature = "native-decode"))]
use movie::MovieCursor;
#[cfg(feature = "native-decode")]
use native::MovieCursor;

pub(crate) struct IndexedVideo {
    backend: Backend,
    size: [u32; 2],
    index: PresentationIndex,
    rgba_len: usize,
    key: Arc<PathBuf>,
}

enum Backend {
    Movie(movie::Movie),
    Animation(Arc<animation::Animation>),
}

/// Decode backends emit display-oriented, encoded-sRGB RGBA8. Alpha is
/// unassociated where present; a proven opaque frame can take the fast GPU path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameAlpha {
    Opaque,
    Straight,
}

#[derive(Clone, Debug)]
pub(crate) struct DecodedFrame {
    pub(crate) rgba: Arc<Vec<u8>>,
    pub(crate) alpha: FrameAlpha,
}

enum Cursor {
    Movie(MovieCursor),
    Animation(animation::Cursor),
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
    key: Arc<PathBuf>,
    decoder: Cursor,
    retained_bytes: usize,
}

struct CachedFrame {
    source: Arc<PathBuf>,
    ordinal: u32,
    frame: DecodedFrame,
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
                .any(|key| Arc::ptr_eq(key, &c.key))
        });
    }

    pub(crate) fn read(
        &mut self,
        source: &IndexedVideo,
        ordinal: u32,
    ) -> Result<(DecodedFrame, bool)> {
        if let Some(i) = self
            .recent
            .iter()
            .position(|f| Arc::ptr_eq(&f.source, &source.key) && f.ordinal == ordinal)
        {
            let hit = self
                .recent
                .remove(i)
                .context("missing cached video frame")?;
            let frame = hit.frame.clone();
            self.recent.push_back(hit);
            return Ok((frame, true));
        }
        let cursor_limit = self.active_profiles.len().max(1);
        let active = self.take_cursor(source, cursor_limit);
        // A small thumbnail can require a full-resolution animation canvas.
        // Evict parked cursors before decoding if a new canvas needs their budget.
        let working_bytes = match &source.backend {
            Backend::Animation(animation) => animation.cursor_bytes(),
            Backend::Movie(_) => 0,
        };
        let mut parked: usize = self.cursors.iter().map(|c| c.retained_bytes).sum();
        while parked > self.limit.saturating_sub(working_bytes) {
            let evicted = self.cursors.pop_front().context("missing parked decoder")?;
            parked -= evicted.retained_bytes;
        }
        #[cfg(feature = "native-decode")]
        let preroll = match (&source.backend, &active) {
            (Backend::Movie(movie), Some(Cursor::Movie(cursor)))
                if cursor.is_near(movie, ordinal) =>
            {
                (self.preroll_limit / cursor_limit).min(self.preroll_limit.saturating_sub(parked))
            }
            _ => 0,
        };
        #[cfg(not(feature = "native-decode"))]
        let preroll = 0;
        let retained_bytes = working_bytes + preroll;
        let rgba_budget = self.limit.saturating_sub(parked + retained_bytes);
        // Make room before handing buffer ownership to a decoder. Distant
        // jumps lend the entire budget back to the requested-frame LRU.
        let keep = rgba_budget
            .checked_sub(source.rgba_len)
            .unwrap_or(rgba_budget);
        while self.bytes > keep || self.recent.len() >= 64 {
            let evicted = self.recent.pop_front().context("missing LRU video frame")?;
            self.bytes -= evicted.frame.rgba.len();
        }
        let (pixels, decoder) = source.decode(ordinal, preroll, active)?;
        let frame = DecodedFrame {
            rgba: Arc::new(pixels),
            alpha: source.alpha(),
        };
        // Large canvases can still yield a small tile. Release such a cursor
        // after this read instead of pinning its canvas to the worker.
        let cache_budget = if retained_bytes <= self.limit {
            self.cursors.push_back(ReaderCursor {
                key: source.key.clone(),
                decoder,
                retained_bytes,
            });
            rgba_budget
        } else {
            drop(decoder);
            self.limit
        };
        if frame.rgba.len() <= cache_budget {
            self.bytes += frame.rgba.len();
            self.recent.push_back(CachedFrame {
                source: source.key.clone(),
                ordinal,
                frame: frame.clone(),
            });
        }
        Ok((frame, false))
    }

    fn take_cursor(&mut self, source: &IndexedVideo, limit: usize) -> Option<Cursor> {
        if let Some(i) = self
            .cursors
            .iter()
            .position(|c| source.has_identity(&c.key))
        {
            return self.cursors.remove(i).map(|entry| entry.decoder);
        }
        if self.cursors.len() >= limit {
            // Beyond the session cap, additional profiles share the last slot.
            // A cyclic scan must not evict every warm cursor on every picture.
            let pinned = &self.active_profiles[..limit - 1];
            let spare = self
                .cursors
                .iter()
                .position(|c| !pinned.iter().any(|key| Arc::ptr_eq(key, &c.key)));
            self.cursors.remove(spare.unwrap_or(0));
        }
        None
    }

    pub(crate) fn retain_sources(&mut self, keep: impl Fn(&Arc<PathBuf>) -> bool) {
        self.recent.retain(|f| keep(&f.source));
        self.bytes = self.recent.iter().map(|f| f.frame.rgba.len()).sum();
        self.cursors.retain(|c| keep(&c.key));
    }
}

impl IndexedVideo {
    pub(crate) fn has_identity(&self, key: &Arc<PathBuf>) -> bool {
        Arc::ptr_eq(&self.key, key)
    }

    pub(crate) fn open(path: &Path, width: u32, height: u32, fps: FrameRate) -> Result<Self> {
        if let Some(animation) = animation::Animation::open(path)? {
            Self::from_animation(path, width, height, fps, animation)
        } else {
            Self::open_movie(path, width, height, fps)
        }
    }

    pub(crate) fn open_movie(path: &Path, width: u32, height: u32, fps: FrameRate) -> Result<Self> {
        let rgba_len = validate_profile(width, height, fps)?;
        let (movie, index) = movie::Movie::open(path, width, height, fps)?;
        Ok(Self {
            backend: Backend::Movie(movie),
            size: [width, height],
            index,
            rgba_len,
            key: Arc::new(path.to_owned()),
        })
    }

    pub(crate) fn from_animation(
        path: &Path,
        width: u32,
        height: u32,
        fps: FrameRate,
        animation: Arc<animation::Animation>,
    ) -> Result<Self> {
        let rgba_len = validate_profile(width, height, fps)?;
        ensure!(
            width <= MAX_SURFACE_DIMENSION && height <= MAX_SURFACE_DIMENSION,
            "invalid animated image output size {width}x{height} (maximum {MAX_SURFACE_DIMENSION} per axis)"
        );
        check_aspect_ratio(animation.size(), [width, height])?;
        Ok(Self {
            index: animation.index(fps)?,
            backend: Backend::Animation(animation),
            size: [width, height],
            rgba_len,
            key: Arc::new(path.to_owned()),
        })
    }

    pub(crate) fn alpha(&self) -> FrameAlpha {
        match self.backend {
            Backend::Movie(_) => FrameAlpha::Opaque,
            Backend::Animation(_) => FrameAlpha::Straight,
        }
    }

    fn decode(
        &self,
        ordinal: u32,
        preroll: usize,
        active: Option<Cursor>,
    ) -> Result<(Vec<u8>, Cursor)> {
        match &self.backend {
            Backend::Movie(movie) => {
                let mut cursor = match active {
                    Some(Cursor::Movie(cursor)) => Some(cursor),
                    None => None,
                    _ => anyhow::bail!("decode cursor belongs to a different source"),
                };
                let pixels =
                    movie.decode(&self.index, self.rgba_len, ordinal, preroll, &mut cursor)?;
                Ok((
                    pixels,
                    Cursor::Movie(cursor.context("missing decoded movie cursor")?),
                ))
            }
            Backend::Animation(animation) => {
                let mut cursor = match active {
                    Some(Cursor::Animation(cursor)) => cursor,
                    None => animation.cursor(self.size)?,
                    _ => anyhow::bail!("decode cursor belongs to a different source"),
                };
                let pixels = cursor.read(ordinal)?;
                Ok((pixels, Cursor::Animation(cursor)))
            }
        }
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
        ensure!(
            self.alpha() == proxy.alpha(),
            "proxy alpha representation differs from the original source"
        );
        // A timed-image proxy could share dimensions and timings while discarding
        // source coverage. Until an alpha-preserving proxy is qualified, draw originals.
        ensure!(
            self.alpha() == FrameAlpha::Opaque,
            "preview proxies for transparent footage are unsupported"
        );
        self.index.check_proxy(&proxy.index)
    }
}

fn validate_profile(width: u32, height: u32, fps: FrameRate) -> Result<usize> {
    ensure!(
        width > 0 && height > 0 && fps.0 > 0 && fps.1 > 0,
        "invalid video profile"
    );
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("video frame is too large")
}

fn check_aspect_ratio([source_w, source_h]: [u32; 2], [width, height]: [u32; 2]) -> Result<()> {
    let expected = u64::from(height) * u64::from(source_w);
    let actual = u64::from(width) * u64::from(source_h);
    // A strip fixes width, while a placement thumbnail fixes the longest side.
    // Either scaled axis can therefore be rounded to the nearest pixel.
    ensure!(
        expected.abs_diff(actual) <= u64::from(source_w.max(source_h)) / 2,
        "source and output aspect ratios differ; explicit framing is required"
    );
    Ok(())
}

/// Positive exposure intervals map back to encoded frames; a zero-delay
/// frame still participates in the decoder's blend and disposal sequence.
struct AnimationTimeline {
    starts_ms: Arc<[i64]>,
    encoded_ordinals: Vec<u32>,
    end_ms: i64,
}

impl AnimationTimeline {
    fn new(durations_ms: &[u32]) -> Result<Self> {
        let mut starts = Vec::new();
        let mut encoded_ordinals = Vec::new();
        let mut end_ms = 0_i64;
        for (ordinal, &duration) in durations_ms.iter().enumerate() {
            if duration > 0 {
                starts.push(end_ms);
                encoded_ordinals.push(u32::try_from(ordinal)?);
                end_ms = end_ms
                    .checked_add(i64::from(duration))
                    .context("animation duration overflow")?;
            }
        }
        ensure!(end_ms > 0, "animation needs a positive display duration");
        Ok(Self {
            starts_ms: starts.into(),
            encoded_ordinals,
            end_ms,
        })
    }

    fn at_rate(&self, fps: FrameRate) -> Result<PresentationIndex> {
        let duration = (i128::from(self.end_ms), 1000);
        Ok(PresentationIndex {
            pts: self.starts_ms.clone(),
            time_base: (1, 1000),
            cfr: None,
            fps,
            ticks: project_tick_count(duration, fps)?,
            sample_end: duration,
        })
    }
}

fn project_tick_count((num, den): (i128, i128), fps: FrameRate) -> Result<u32> {
    let numerator = num * i128::from(fps.0);
    let denominator = den * i128::from(fps.1);
    u32::try_from((numerator + denominator - 1) / denominator)
        .context("duration exceeds source tick domain")
}

/// Convert between source PTS and project-rate ticks using integer arithmetic.
/// Quantized CFR timestamps (e.g. 33/34 ms at 30 fps) use their verified native
/// frame grid. A variable frame is held until the next frame's actual PTS.
struct PresentationIndex {
    pts: Arc<[i64]>,
    time_base: (i128, i128),
    cfr: Option<(i128, i128)>,
    fps: FrameRate,
    ticks: u32,
    /// End of the last displayed sample in seconds, relative to the first PTS.
    /// Use the verified CFR span only when no sample or stream duration exists.
    sample_end: (i128, i128),
}

impl PresentationIndex {
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

#[cfg(test)]
mod tests;
