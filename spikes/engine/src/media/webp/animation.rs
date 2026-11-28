//! A revision owns immutable compressed bytes and exact frame holds. Each worker owns a
//! libwebp cursor that reconstructs full straight-RGBA canvases, including disposal.
use super::{MAX_ICC_BYTES, apply_profile, check_dimensions, srgb_transform};
use crate::media::animation::{Codec, DecodeCursor};
use anyhow::{Context, Result, ensure};
use moxcms::{Layout, Transform8BitExecutor};
use std::{
    borrow::Cow,
    ffi::{c_int, c_void},
    ptr::NonNull,
    sync::Arc,
};

const MAX_FRAMES: usize = 65_535;

pub(in crate::media) struct Animation {
    bytes: Box<[u8]>,
    size: [u32; 2],
    durations: Vec<u32>,
    transform: Option<Arc<Transform8BitExecutor>>,
    #[allow(dead_code)] // Preserved source metadata; the editable clip plays one pass.
    loop_count: u16,
}

impl Animation {
    pub(in crate::media) fn parse(bytes: Box<[u8]>) -> Result<Arc<Self>> {
        ensure!(
            bytes.starts_with(b"RIFF")
                && bytes.get(8..12) == Some(b"WEBP")
                && u32::from_le_bytes(bytes[4..8].try_into()?) as usize + 8 == bytes.len(),
            "invalid WebP RIFF header"
        );
        let mut chunks = &bytes[12..];
        let (kind, header) = next_chunk(&mut chunks)?;
        ensure!(kind == b"VP8X", "still WebP requires an <image> layer");
        ensure!(header.len() == 10, "invalid WebP extended header size");
        ensure!(header[0] & 2 != 0, "still WebP requires an <image> layer");
        let size = [u24(&header[4..7]) + 1, u24(&header[7..10]) + 1];
        check_dimensions(size[0], size[1])?;
        let mut icc = None;
        let mut loop_count = None;
        let mut durations = Vec::new();
        let mut auxiliary_chunks = 0;
        let mut end_ms = 0i32; // libwebp's cumulative timestamp is a signed integer.
        while !chunks.is_empty() {
            let (kind, data) = next_chunk(&mut chunks)?;
            match kind {
                b"VP8X" => anyhow::bail!("duplicate WebP extended header"),
                b"ANIM" => {
                    ensure!(loop_count.is_none() && data.len() == 6, "invalid WebP ANIM");
                    loop_count = Some(u16::from_le_bytes(data[4..].try_into()?));
                }
                b"ICCP" => {
                    ensure!(
                        icc.is_none() && data.len() <= MAX_ICC_BYTES,
                        "invalid WebP ICC profile size or duplicate ICCP"
                    );
                    icc = Some(data);
                }
                b"ANMF" => {
                    ensure!(
                        loop_count.is_some() && data.len() >= 24,
                        "invalid WebP animation frame header"
                    );
                    ensure!(
                        durations.len() < MAX_FRAMES,
                        "WebP exceeds {MAX_FRAMES} frames"
                    );
                    let x = u24(&data[..3]) * 2;
                    let y = u24(&data[3..6]) * 2;
                    let w = u24(&data[6..9]) + 1;
                    let h = u24(&data[9..12]) + 1;
                    ensure!(
                        x + w <= size[0] && y + h <= size[1],
                        "WebP frame exceeds its logical canvas"
                    );
                    check_plane(&data[16..], [w, h])?;
                    let ms = u24(&data[12..15]);
                    end_ms = end_ms
                        .checked_add(ms as i32)
                        .context("WebP source duration exceeds decoder timestamp limit")?;
                    durations.push(ms);
                }
                _ => {
                    // libwebp builds a demux entry for each retained ancillary chunk.
                    auxiliary_chunks += 1;
                    ensure!(auxiliary_chunks <= 1024, "too many WebP ancillary chunks");
                }
            }
        }
        ensure!(
            end_ms > 0,
            "animated WebP needs a positive display duration"
        );
        ensure!(
            (header[0] & 0x20 != 0) == icc.is_some(),
            "inconsistent WebP ICC profile flag"
        );
        let transform = icc
            .map(|profile| srgb_transform(profile, Layout::Rgba))
            .transpose()?;
        let animation = Arc::new(Self {
            bytes,
            size,
            durations,
            transform,
            loop_count: loop_count.context("missing WebP ANIM")?,
        });
        // Validate every compressed frame before accepting a revision. The
        // temporary cursor walks forward without copying the reconstructed canvases.
        let mut cursor = animation.cursor()?;
        for _ in &animation.durations {
            cursor.advance()?;
        }
        Ok(animation)
    }

    fn cursor(self: &Arc<Self>) -> Result<AnimationCursor> {
        // SAFETY: self.bytes is immutable and the cursor retains the Arc until deletion.
        let decoder = unsafe {
            n4_webp_animation_open(
                self.bytes.as_ptr(),
                self.bytes.len(),
                self.size[0],
                self.size[1],
                self.durations.len() as u32,
            )
        };
        Ok(AnimationCursor {
            decoder: NonNull::new(decoder).context("invalid WebP animation bitstream")?,
            animation: self.clone(),
            next: 0,
            #[cfg(test)]
            frames_decoded: 0,
        })
    }
}

impl Codec for Animation {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn durations(&self) -> &[u32] {
        &self.durations
    }

    fn encoded_bytes(&self) -> &[u8] {
        &self.bytes
    }

    // libwebp retains the displayed and disposed canvases.
    fn cursor_bytes(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize * 4 * 2
    }

    fn new_cursor(self: Arc<Self>) -> Result<Box<dyn DecodeCursor>> {
        Ok(Box::new(self.cursor()?))
    }
}

/// Check the dimensions advertised by the compressed plane against its ANMF rectangle
/// before handing it to a decoder. The two headers are independently authored.
fn check_plane(mut payload: &[u8], expected: [u32; 2]) -> Result<()> {
    let (mut kind, mut plane) = next_chunk(&mut payload)?;
    if kind == b"ALPH" {
        (kind, plane) = next_chunk(&mut payload)?;
        ensure!(kind == b"VP8 ", "WebP alpha plane requires a VP8 image");
    }
    let actual = match kind {
        b"VP8 " => super::lossy_plane_size(plane)?,
        b"VP8L" => {
            ensure!(
                plane.len() >= 5 && plane[0] == 0x2f,
                "invalid VP8L frame header"
            );
            let bits = u32::from_le_bytes(plane[1..5].try_into()?);
            [(bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1]
        }
        _ => anyhow::bail!("missing compressed WebP frame"),
    };
    ensure!(
        actual == expected,
        "WebP compressed frame size differs from its rectangle"
    );
    Ok(())
}

fn u24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn next_chunk<'a>(remaining: &mut &'a [u8]) -> Result<(&'a [u8], &'a [u8])> {
    let (header, rest) = remaining
        .split_at_checked(8)
        .context("truncated WebP chunk header")?;
    let len = u32::from_le_bytes(header[4..].try_into()?) as usize;
    let (padded, tail) = rest
        .split_at_checked(len + (len & 1))
        .context("truncated WebP chunk payload")?;
    *remaining = tail;
    Ok((&header[..4], &padded[..len]))
}

unsafe extern "C" {
    fn n4_webp_animation_open(
        bytes: *const u8,
        len: usize,
        w: u32,
        h: u32,
        frames: u32,
    ) -> *mut c_void;
    fn WebPAnimDecoderGetNext(
        decoder: *mut c_void,
        rgba: *mut *mut u8,
        end_ms: *mut c_int,
    ) -> c_int;
    fn WebPAnimDecoderReset(decoder: *mut c_void);
    fn WebPAnimDecoderDelete(decoder: *mut c_void);
}

struct AnimationCursor {
    decoder: NonNull<c_void>,
    animation: Arc<Animation>,
    next: u32,
    #[cfg(test)]
    frames_decoded: u64,
}

// SAFETY: the decoder has no thread affinity and is exclusively owned by this cursor.
unsafe impl Send for AnimationCursor {}

impl Drop for AnimationCursor {
    fn drop(&mut self) {
        // SAFETY: this cursor owns the handle and its source still lives in self.animation.
        unsafe { WebPAnimDecoderDelete(self.decoder.as_ptr()) };
    }
}

impl AnimationCursor {
    fn advance(&mut self) -> Result<NonNull<u8>> {
        let mut rgba = std::ptr::null_mut();
        let mut end_ms = 0;
        // SAFETY: the decoder is exclusively held. The caller consumes the borrowed canvas
        // before advancing or resetting the cursor, which would invalidate the pointer.
        let ok = unsafe { WebPAnimDecoderGetNext(self.decoder.as_ptr(), &mut rgba, &mut end_ms) };
        ensure!(ok != 0, "decode WebP animation frame {}", self.next);
        self.next += 1;
        #[cfg(test)]
        {
            self.frames_decoded += 1;
        }
        NonNull::new(rgba).context("WebP decoder returned an empty canvas")
    }
}

impl DecodeCursor for AnimationCursor {
    #[cfg(test)]
    fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    fn canvas(&mut self, ordinal: u32) -> Result<Cow<'_, [u8]>> {
        ensure!(
            (ordinal as usize) < self.animation.durations.len(),
            "WebP frame ordinal out of range"
        );
        if ordinal < self.next {
            // SAFETY: there is no outstanding borrowed canvas.
            unsafe { WebPAnimDecoderReset(self.decoder.as_ptr()) };
            self.next = 0;
        }
        while self.next < ordinal {
            self.advance()?;
        }
        let canvas = self.advance()?;
        let [w, h] = self.animation.size;
        // SAFETY: the constructor verifies the dimensions and straight RGBA8 format.
        // The returned slice borrows this cursor exclusively. Advancing, resetting or
        // dropping the native decoder is impossible until the caller releases that borrow.
        let pixels =
            unsafe { std::slice::from_raw_parts(canvas.as_ptr(), w as usize * h as usize * 4) };
        if let Some(transform) = &self.animation.transform {
            let mut color = pixels.to_vec();
            apply_profile(&**transform, 4, w as usize, &mut color)?;
            Ok(Cow::Owned(color))
        } else {
            Ok(Cow::Borrowed(pixels))
        }
    }
}

#[cfg(test)]
#[path = "animation_tests.rs"]
mod tests;
