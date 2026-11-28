//! GIF owns indexed-image decoding and reconstruction of the logical screen.
//! Workers receive an accepted byte snapshot and return complete straight RGBA8 canvases.
use super::animation::{Codec, DecodeCursor};
use crate::{MAX_SURFACE_DIMENSION, zeroed_bytes};
use anyhow::{Context, Result, ensure};
use gif::{DecodeOptions, Decoder, DisposalMethod, MemoryLimit, Repeat};
use std::{
    borrow::Cow,
    io::{Cursor, Read},
    sync::Arc,
};

const MAX_IMAGES: usize = 65_535;
const DEFAULT_HOLD_MS: u32 = 100;
const METADATA_LIMIT: u64 = 4 * 1024 * 1024;

pub(crate) struct Animation {
    bytes: Box<[u8]>,
    size: [u32; 2],
    holds_ms: Vec<u32>,
    cursor_bytes: usize,
    #[allow(dead_code)] // The authored placement uses one pass regardless of source metadata.
    repeat: Repeat,
}

impl Animation {
    pub(crate) fn parse(bytes: Box<[u8]>) -> Result<Arc<Self>> {
        let mut stream = CanvasDecoder::new(bytes.as_ref())?;
        let mut holds_ms = Vec::new();
        while let Some(hold) = stream.advance()? {
            holds_ms.push(hold);
        }
        ensure!(!holds_ms.is_empty(), "GIF contains no images");
        ensure!(
            holds_ms.len() > 1,
            "single-image GIF requires an <image> layer"
        );
        let size = stream.size;
        let cursor_bytes = stream.retained_bytes();
        let repeat = stream.reader.repeat();
        stream.finish()?;
        Ok(Arc::new(Self {
            bytes,
            size,
            holds_ms,
            cursor_bytes,
            repeat,
        }))
    }
}

impl Codec for Animation {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn durations(&self) -> &[u32] {
        &self.holds_ms
    }

    fn encoded_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn cursor_bytes(&self) -> usize {
        self.cursor_bytes
    }

    fn new_cursor(self: Arc<Self>) -> Result<Box<dyn DecodeCursor>> {
        Ok(Box::new(AnimationCursor::new(self)?))
    }
}

/// Single-image GIF uses the identical canvas rules. Its transparent pixels are
/// transparent black, so the binary alpha is also valid for premultiplied stills.
pub(crate) fn decode_still(bytes: Box<[u8]>) -> Result<([u32; 2], Vec<u8>)> {
    let mut stream = CanvasDecoder::new(bytes.as_ref())?;
    ensure!(stream.advance()?.is_some(), "GIF contains no images");
    ensure!(
        stream.reader.next_frame_info()?.is_none(),
        "animated GIF requires a <video> layer"
    );
    let size = stream.size;
    Ok((size, stream.finish()?))
}

// Give std::io::Cursor a shared, seekable view of the revision without copying its payload.
#[derive(Clone)]
struct SourceBytes(Arc<Animation>);

impl AsRef<[u8]> for SourceBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0.bytes
    }
}

struct AnimationCursor {
    source: SourceBytes,
    stream: CanvasDecoder<Cursor<SourceBytes>>,
    #[cfg(test)]
    decoded: u64,
}

impl AnimationCursor {
    fn new(animation: Arc<Animation>) -> Result<Self> {
        let source = SourceBytes(animation);
        let stream = CanvasDecoder::new(Cursor::new(source.clone()))?;
        Ok(Self {
            source,
            stream,
            #[cfg(test)]
            decoded: 0,
        })
    }
}

impl DecodeCursor for AnimationCursor {
    fn canvas(&mut self, ordinal: u32) -> Result<Cow<'_, [u8]>> {
        ensure!(
            (ordinal as usize) < self.source.0.holds_ms.len(),
            "GIF frame ordinal out of range"
        );
        if ordinal < self.stream.next {
            self.stream.reader = decoder(Cursor::new(self.source.clone()))?;
            self.stream.next = 0;
            self.stream.disposal = Disposal::Keep;
        }
        while self.stream.next <= ordinal {
            self.stream.advance()?.context("missing GIF image")?;
            #[cfg(test)]
            {
                self.decoded += 1;
            }
        }
        Ok(Cow::Borrowed(&self.stream.canvas))
    }

    #[cfg(test)]
    fn frames_decoded(&self) -> u64 {
        self.decoded
    }
}

fn check_size(size: [u32; 2]) -> Result<()> {
    ensure!(
        size.iter().all(|n| (1..=MAX_SURFACE_DIMENSION).contains(n)),
        "invalid GIF dimensions {}x{} (maximum {MAX_SURFACE_DIMENSION} per axis)",
        size[0],
        size[1]
    );
    Ok(())
}

fn decoder<R: Read>(input: R) -> Result<Decoder<R>> {
    let mut options = DecodeOptions::new(); // Indexed output. N4 resolves transparency during disposal and draw.
    options.check_frame_consistency(true);
    options.check_lzw_end_code(true);
    // Indexed frame storage is externally provided and bounded by the logical screen.
    // This limit governs the decoder's internal allocations, including XMP/ICC extensions.
    options.set_memory_limit(MemoryLimit::Bytes(METADATA_LIMIT.try_into()?));
    Ok(options.read_info(input)?)
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn rows(self, canvas: &mut [u8], stride: usize) -> impl Iterator<Item = &mut [u8]> {
        canvas
            .chunks_exact_mut(stride)
            .skip(self.y)
            .take(self.h)
            .map(move |row| &mut row[self.x * 4..(self.x + self.w) * 4])
    }
}

enum Disposal {
    Keep,
    Background(Rect, [u8; 4]),
    Previous(Rect),
}

struct CanvasDecoder<R: Read> {
    reader: Decoder<R>,
    size: [u32; 2],
    background: [u8; 4],
    canvas: Vec<u8>,
    indices: Vec<u8>,
    // Only the affected rectangle needs saving for restore-previous.
    saved: Vec<u8>,
    disposal: Disposal,
    next: u32,
}

impl<R: Read> CanvasDecoder<R> {
    fn new(input: R) -> Result<Self> {
        let reader = decoder(input)?;
        let size = [u32::from(reader.width()), u32::from(reader.height())];
        check_size(size)?;
        let background = reader
            .bg_color()
            .and_then(|index| reader.global_palette()?.as_chunks::<3>().0.get(index))
            .map_or([0; 4], |&[r, g, b]| [r, g, b, 255]);
        Ok(Self {
            reader,
            size,
            background,
            canvas: zeroed_bytes(size[0] as usize * size[1] as usize * 4)?,
            indices: Vec::new(),
            saved: Vec::new(),
            disposal: Disposal::Keep,
            next: 0,
        })
    }

    fn advance(&mut self) -> Result<Option<u32>> {
        let Some(frame) = self.reader.next_frame_info()? else {
            return Ok(None);
        };
        ensure!(
            (self.next as usize) < MAX_IMAGES,
            "GIF exceeds {MAX_IMAGES} images"
        );
        ensure!(
            !frame.needs_user_input,
            "interactive GIF requests user input"
        );
        let rect = Rect {
            x: frame.left.into(),
            y: frame.top.into(),
            w: frame.width.into(),
            h: frame.height.into(),
        };
        ensure!(rect.w > 0 && rect.h > 0, "empty GIF image rectangle");
        let transparent = frame.transparent;
        let disposal = frame.dispose;
        let hold = match frame.delay {
            0 => DEFAULT_HOLD_MS,
            centiseconds => u32::from(centiseconds) * 10,
        };

        let pixels = rect.w * rect.h;
        self.indices
            .try_reserve_exact(pixels.saturating_sub(self.indices.len()))?;
        self.indices.resize(pixels, 0);
        self.reader.read_into_buffer(&mut self.indices)?; // Deinterlaces into screen row order.
        ensure!(
            !self.reader.fill_buffer(&mut [0])?,
            "GIF image data exceeds its rectangle"
        );

        let stride = self.size[0] as usize * 4;
        self.prepare_canvas(rect, transparent, disposal, stride)?;
        let palette = self.reader.palette()?.as_chunks::<3>().0;
        ensure!(
            transparent.is_none_or(|index| usize::from(index) < palette.len()),
            "invalid GIF transparent palette index"
        );
        for (row, indices) in rect
            .rows(&mut self.canvas, stride)
            .zip(self.indices.chunks_exact(rect.w))
        {
            for (dst, &index) in row.as_chunks_mut::<4>().0.iter_mut().zip(indices) {
                let &[r, g, b] = palette
                    .get(usize::from(index))
                    .context("invalid GIF pixel palette index")?;
                if Some(index) != transparent {
                    *dst = [r, g, b, 255];
                }
            }
        }
        self.next += 1;
        Ok(Some(hold))
    }

    // Dispose the preceding image, then preserve the pixels the incoming image may replace.
    fn prepare_canvas(
        &mut self,
        rect: Rect,
        transparent: Option<u8>,
        disposal: DisposalMethod,
        stride: usize,
    ) -> Result<()> {
        let background = if transparent.is_some() {
            [0; 4]
        } else {
            self.background
        };
        if self.next == 0 {
            fill(&mut self.canvas, background);
        } else {
            match self.disposal {
                Disposal::Keep => {}
                Disposal::Background(area, color) => {
                    for row in area.rows(&mut self.canvas, stride) {
                        fill(row, color);
                    }
                }
                Disposal::Previous(area) => {
                    for (dst, src) in area
                        .rows(&mut self.canvas, stride)
                        .zip(self.saved.chunks_exact(area.w * 4))
                    {
                        dst.copy_from_slice(src);
                    }
                }
            }
        }
        // Save the destination underneath this frame before drawing it.
        self.disposal = match disposal {
            DisposalMethod::Any | DisposalMethod::Keep => Disposal::Keep,
            DisposalMethod::Background => Disposal::Background(rect, background),
            DisposalMethod::Previous => {
                self.saved.clear();
                self.saved.try_reserve_exact(rect.w * rect.h * 4)?;
                for row in rect.rows(&mut self.canvas, stride) {
                    self.saved.extend_from_slice(row);
                }
                Disposal::Previous(rect)
            }
        };
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>> {
        // The decoder stops at the first GIF trailer. An asset represents one data stream.
        ensure!(
            self.reader.into_inner().read(&mut [0])? == 0,
            "unexpected data after GIF trailer"
        );
        Ok(self.canvas)
    }

    fn retained_bytes(&self) -> usize {
        // Cursor-specific high-water allocations observed during validation. Metadata Vecs
        // may reserve geometrically. 64 KiB covers the 12-bit LZW table, IO buffer and palettes.
        let metadata = self.reader.icc_profile().map_or(0, <[u8]>::len)
            + self.reader.xmp_metadata().map_or(0, <[u8]>::len);
        self.canvas.capacity()
            + self.indices.capacity()
            + self.saved.capacity()
            + 2 * metadata
            + 64 * 1024
    }
}

fn fill(bytes: &mut [u8], color: [u8; 4]) {
    bytes.as_chunks_mut::<4>().0.fill(color);
}

#[cfg(test)]
#[path = "gif/tests.rs"]
mod tests;
