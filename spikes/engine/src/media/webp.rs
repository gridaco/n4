use super::image_file::ImageFile;
use crate::{MAX_SURFACE_DIMENSION, zeroed_bytes};
use anyhow::{Context, Result, ensure};
use image_webp::WebPDecoder;
use moxcms::{
    ColorProfile, DataColorSpace, Layout, RenderingIntent, Transform8BitExecutor, TransformOptions,
};
use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

mod animation;
pub(super) use animation::Animation;

const MAX_ICC_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct Still(WebPDecoder<BufReader<DecoderInput>>);

/// image-webp 0.2 selects RGB output from the header and drops the decoded alpha.
/// For a lossless payload, request RGBA in its view of the header. The compressed
/// pixels and the source file stay intact. Keep reads and size probes streaming.
struct DecoderInput {
    file: File,
    position: u64,
    alpha_flag: Option<u64>,
}

impl Read for DecoderInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.file.read(buffer)?;
        if let Some(at) = self.alpha_flag
            && (self.position..self.position + count as u64).contains(&at)
        {
            buffer[(at - self.position) as usize] |= 0x10;
        }
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for DecoderInput {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = self.file.seek(position)?;
        Ok(self.position)
    }
}

/// The codec reads VP8 plane dimensions before comparing them to the VP8X
/// canvas. Check chunk extents and those plane dimensions before pixel allocation.
fn check_container(reader: &mut BufReader<File>, end: u64) -> Result<Option<u64>> {
    let mut offset = 12;
    let mut expects_icc = false;
    let mut has_icc = false;
    let mut extended = false;
    let mut alpha_flag = None;
    while offset < end {
        let first = offset == 12;
        let mut header = [0; 8];
        reader.read_exact(&mut header)?;
        let len = u64::from(u32::from_le_bytes(header[4..8].try_into()?));
        offset += 8 + len + (len & 1);
        ensure!(offset <= end, "truncated WebP chunk");
        let inspected = match &header[..4] {
            b"VP8X" => {
                extended |= first;
                ensure!(len == 10, "invalid WebP extended header size");
                let mut flags = [0];
                reader.read_exact(&mut flags)?;
                ensure!(flags[0] & 2 == 0, "animated WebP requires a <video> layer");
                expects_icc = flags[0] & 0x20 != 0;
                1
            }
            b"VP8 " => {
                ensure!(len >= 10, "truncated VP8 header");
                let mut frame = [0; 10];
                reader.read_exact(&mut frame)?;
                let [width, height] = lossy_plane_size(&frame)?;
                check_dimensions(width, height)?;
                10
            }
            b"VP8L" => {
                // image-webp selects output channels from VP8X flags on an
                // extended still or from bit 28 of the size word on simple VP8L.
                if first {
                    alpha_flag = Some(24);
                } else if extended {
                    alpha_flag = Some(20);
                }
                0
            }
            b"ICCP" => {
                ensure!(
                    len <= MAX_ICC_BYTES as u64,
                    "WebP ICC profile exceeds 4 MiB"
                );
                ensure!(!has_icc, "duplicate WebP ICC profile");
                has_icc = true;
                0
            }
            b"ANIM" | b"ANMF" => anyhow::bail!("animated WebP requires a <video> layer"),
            _ => 0,
        };
        // Keep small chunks in the BufReader when possible.
        reader.seek_relative((len + (len & 1) - inspected) as i64)?;
    }
    ensure!(expects_icc == has_icc, "inconsistent WebP ICC profile flag");
    reader.rewind()?;
    Ok(alpha_flag)
}

fn lossy_plane_size(header: &[u8]) -> Result<[u32; 2]> {
    ensure!(header.len() >= 10, "truncated VP8 header");
    let w = u16::from_le_bytes([header[6], header[7]]) & 0x3fff;
    let h = u16::from_le_bytes([header[8], header[9]]) & 0x3fff;
    Ok([u32::from(w), u32::from(h)])
}

fn check_dimensions(width: u32, height: u32) -> Result<()> {
    ensure!(
        [width, height]
            .into_iter()
            .all(|n| (1..=MAX_SURFACE_DIMENSION).contains(&n)),
        "invalid WebP dimensions {width}x{height} (maximum {MAX_SURFACE_DIMENSION} per axis)"
    );
    Ok(())
}

impl Still {
    pub(crate) fn open(source: ImageFile) -> Result<Self> {
        let (mut reader, end) = source.into_parts();
        let alpha_flag = check_container(&mut reader, end)?;
        let mut decoder = WebPDecoder::new(BufReader::new(DecoderInput {
            file: reader.into_inner(),
            position: 0,
            alpha_flag,
        }))?;
        ensure!(
            !decoder.is_animated(),
            "animated WebP requires a <video> layer"
        );
        let (width, height) = decoder.dimensions();
        check_dimensions(width, height)?;
        decoder.set_memory_limit(MAX_ICC_BYTES);
        Ok(Self(decoder))
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        let (w, h) = self.0.dimensions();
        [w, h]
    }

    /// Decode premultiplied RGBA8 in encoded sRGB.
    pub(crate) fn decode(self, path: &Path) -> Result<Vec<u8>> {
        self.rgba()
            .with_context(|| format!("decode WebP {}", path.display()))
    }

    fn rgba(mut self) -> Result<Vec<u8>> {
        let size = self.size();
        let rgba_len = (size[0] as usize)
            .checked_mul(size[1] as usize)
            .and_then(|n| n.checked_mul(4))
            .context("WebP raster size overflow")?;
        let source_len = self.0.output_buffer_size().context("WebP size overflow")?;
        let icc = self.0.icc_profile().context("read WebP ICC profile")?;
        let alpha = self.0.has_alpha();
        let mut rgba = zeroed_bytes(rgba_len)?;
        self.0.set_memory_limit(rgba_len);
        self.0.read_image(&mut rgba[..source_len])?;
        if let Some(profile) = icc {
            let layout = if alpha { Layout::Rgba } else { Layout::Rgb };
            let transform = srgb_transform(&profile, layout)?;
            apply_profile(
                &*transform,
                if alpha { 4 } else { 3 },
                size[0] as usize,
                &mut rgba[..source_len],
            )?;
        }
        if alpha {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                let a = u16::from(pixel[3]);
                for color in &mut pixel[..3] {
                    *color = ((u16::from(*color) * a + 127) / 255) as u8;
                }
            }
        } else {
            // Expand the RGB decode in its final allocation, from the end.
            for i in (0..source_len / 3).rev() {
                rgba.copy_within(i * 3..i * 3 + 3, i * 4);
                rgba[i * 4 + 3] = 255;
            }
        }
        Ok(rgba)
    }
}

fn srgb_transform(icc: &[u8], layout: Layout) -> Result<Arc<Transform8BitExecutor>> {
    let profile = ColorProfile::new_from_slice(icc).context("invalid WebP ICC profile")?;
    ensure!(
        profile.color_space == DataColorSpace::Rgb,
        "WebP ICC profile must describe RGB"
    );
    profile
        .create_transform_8bit(
            layout,
            &ColorProfile::new_srgb(),
            layout,
            TransformOptions {
                rendering_intent: RenderingIntent::RelativeColorimetric,
                ..Default::default()
            },
        )
        .context("unsupported WebP ICC profile")
}

fn apply_profile(
    transform: &Transform8BitExecutor,
    channels: usize,
    width: usize,
    pixels: &mut [u8],
) -> Result<()> {
    let mut output = zeroed_bytes(width * channels)?;
    for row in pixels.chunks_exact_mut(output.len()) {
        transform
            .transform(row, &mut output)
            .context("convert WebP ICC color to sRGB")?;
        // Coverage is independent of the ICC transform.
        for (dst, src) in row
            .chunks_exact_mut(channels)
            .zip(output.chunks_exact(channels))
        {
            dst[..3].copy_from_slice(&src[..3]);
        }
    }
    Ok(())
}
