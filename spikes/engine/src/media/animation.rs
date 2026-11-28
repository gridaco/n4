//! A timed image is one immutable source with a shared display clock and per-worker decoders.
use super::{
    AnimationTimeline, PresentationIndex, gif,
    image_file::{ImageFile, ImageFormat},
    webp,
};
use crate::{FrameRate, zeroed_bytes};
use anyhow::{Context, Result};
use std::{
    borrow::Cow,
    path::Path,
    sync::{Arc, OnceLock},
};

/// Container recognition and codec-specific admission end here. Both adapters
/// expose complete straight-sRGB RGBA canvases and millisecond holds.
pub(super) trait Codec: Send + Sync {
    fn size(&self) -> [u32; 2];
    fn durations(&self) -> &[u32];
    fn encoded_bytes(&self) -> &[u8];
    /// Maximum retained decoder working set, including full-size disposal state.
    fn cursor_bytes(&self) -> usize;
    fn new_cursor(self: Arc<Self>) -> Result<Box<dyn DecodeCursor>>;
}

pub(super) trait DecodeCursor: Send {
    /// Reconstruct the full native canvas. The borrow expires before the next seek or decode.
    fn canvas(&mut self, encoded_ordinal: u32) -> Result<Cow<'_, [u8]>>;
    #[cfg(test)]
    fn frames_decoded(&self) -> u64;
}

pub(crate) struct Animation {
    codec: Arc<dyn Codec>,
    timeline: AnimationTimeline,
    digest: OnceLock<u64>,
}

impl Animation {
    pub(super) fn open(path: &Path) -> Result<Option<Arc<Self>>> {
        let Some(file) = ImageFile::open(path)? else {
            return Ok(None);
        };
        let format = file.format;
        Self::parse(file)
            .map(Some)
            .with_context(|| format!("open timed {format} {}", path.display()))
    }

    fn parse(file: ImageFile) -> Result<Arc<Self>> {
        let format = file.format;
        let bytes = file.into_bytes()?;
        let codec: Arc<dyn Codec> = match format {
            ImageFormat::WebP => webp::Animation::parse(bytes)?,
            ImageFormat::Gif => gif::Animation::parse(bytes)?,
        };
        let timeline = AnimationTimeline::new(codec.durations())?;
        Ok(Arc::new(Self {
            codec,
            timeline,
            digest: OnceLock::new(),
        }))
    }

    pub(super) fn size(&self) -> [u32; 2] {
        self.codec.size()
    }

    pub(super) fn index(&self, fps: FrameRate) -> Result<PresentationIndex> {
        self.timeline.at_rate(fps)
    }

    pub(super) fn cursor_bytes(&self) -> usize {
        self.codec.cursor_bytes()
    }

    pub(super) fn cursor(self: &Arc<Self>, target: [u32; 2]) -> Result<Cursor> {
        Ok(Cursor {
            animation: self.clone(),
            decoder: self.codec.clone().new_cursor()?,
            target,
        })
    }

    pub(crate) fn digest(&self) -> u64 {
        // FNV-1a over the accepted file, independent of renderer and build revision.
        *self.digest.get_or_init(|| {
            self.codec
                .encoded_bytes()
                .iter()
                .fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
                })
        })
    }
}

pub(super) struct Cursor {
    animation: Arc<Animation>,
    decoder: Box<dyn DecodeCursor>,
    target: [u32; 2],
}

impl Cursor {
    pub(super) fn read(&mut self, display_ordinal: u32) -> Result<Vec<u8>> {
        let encoded = self
            .animation
            .timeline
            .encoded_ordinals
            .get(display_ordinal as usize)
            .context("animation display ordinal outside source")?;
        let canvas = self.decoder.canvas(*encoded)?;
        let size = self.animation.size();
        if size == self.target {
            Ok(canvas.into_owned())
        } else {
            resize(&canvas, size, self.target)
        }
    }

    #[cfg(test)]
    pub(super) fn frames_decoded(&self) -> u64 {
        self.decoder.frames_decoded()
    }
}

/// Bilinear sampling in linear premultiplied color, returned as straight sRGB RGBA8.
pub(super) fn resize(input: &[u8], size: [u32; 2], target: [u32; 2]) -> Result<Vec<u8>> {
    fn neighbors(out: usize, src_len: usize, dst_len: usize) -> (usize, usize, f32) {
        let p = ((out as f32 + 0.5) * src_len as f32 / dst_len as f32 - 0.5)
            .clamp(0.0, (src_len - 1) as f32);
        let first = p.floor() as usize;
        (first, (first + 1).min(src_len - 1), p - first as f32)
    }
    static LINEAR: OnceLock<[f32; 256]> = OnceLock::new();
    let linear = LINEAR.get_or_init(|| {
        std::array::from_fn(|i| {
            let s = i as f32 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    let [w, h] = size.map(|n| n as usize);
    let [tw, th] = target.map(|n| n as usize);
    let mut output = zeroed_bytes(tw * th * 4)?;
    for y in 0..th {
        let (y0, y1, fy) = neighbors(y, h, th);
        for x in 0..tw {
            let (x0, x1, fx) = neighbors(x, w, tw);
            let mut associated = [0.0; 3];
            let mut alpha = 0.0;
            for (sx, sy, weight) in [
                (x0, y0, (1.0 - fx) * (1.0 - fy)),
                (x1, y0, fx * (1.0 - fy)),
                (x0, y1, (1.0 - fx) * fy),
                (x1, y1, fx * fy),
            ] {
                let pixel = &input[(sy * w + sx) * 4..][..4];
                let coverage = weight * f32::from(pixel[3]);
                alpha += coverage;
                for c in 0..3 {
                    associated[c] += coverage * linear[pixel[c] as usize];
                }
            }
            if alpha > 0.0 {
                let dst = &mut output[(y * tw + x) * 4..][..4];
                for c in 0..3 {
                    let l = associated[c] / alpha;
                    let encoded = if l <= 0.0031308 {
                        12.92 * l
                    } else {
                        1.055 * l.powf(1.0 / 2.4) - 0.055
                    };
                    dst[c] = (255.0 * encoded).round() as u8;
                }
                dst[3] = alpha.round() as u8;
            }
        }
    }
    Ok(output)
}
