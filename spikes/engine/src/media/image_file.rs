//! Recognize engine-owned image formats from the same open file used for decoding.
use crate::zeroed_bytes;
use anyhow::{Context, Result, ensure};
use std::{
    fmt,
    fs::File,
    io::{BufRead, BufReader, ErrorKind, Read, Seek},
    path::Path,
};

const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) enum ImageFormat {
    WebP,
    Gif,
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WebP => "WebP",
            Self::Gif => "GIF",
        })
    }
}

pub(crate) struct ImageFile {
    pub(crate) format: ImageFormat,
    reader: BufReader<File>,
    size: u64,
}

impl ImageFile {
    pub(crate) fn open(path: &Path) -> Result<Option<Self>> {
        Self::recognize(path).with_context(|| format!("read media {}", path.display()))
    }

    fn recognize(path: &Path) -> Result<Option<Self>> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut header = [0; 12];
        // A six-byte GIF signature commits even a truncated file to its decoder.
        if !read_signature(&mut reader, &mut header[..6])? {
            return Ok(None);
        }
        let format = if matches!(&header[..6], b"GIF87a" | b"GIF89a") {
            ImageFormat::Gif
        } else if &header[..4] == b"RIFF"
            && read_signature(&mut reader, &mut header[6..])?
            && &header[8..] == b"WEBP"
        {
            ImageFormat::WebP
        } else {
            return Ok(None);
        };
        let size = reader.get_ref().metadata()?.len();
        ensure!(
            size <= MAX_FILE_BYTES,
            "{format} exceeds 256 MiB encoded limit"
        );
        if let ImageFormat::WebP = format {
            let end = u64::from(u32::from_le_bytes(header[4..8].try_into()?)) + 8;
            ensure!(end == size, "invalid WebP RIFF length");
        }
        Ok(Some(Self {
            format,
            reader,
            size,
        }))
    }

    /// The stream is positioned immediately after the recognized signature.
    pub(super) fn into_parts(self) -> (BufReader<File>, u64) {
        (self.reader, self.size)
    }

    pub(crate) fn into_bytes(mut self) -> Result<Box<[u8]>> {
        self.reader.rewind()?;
        let mut bytes = zeroed_bytes(usize::try_from(self.size)?)?;
        self.reader.read_exact(&mut bytes)?;
        ensure!(
            self.reader.fill_buf()?.is_empty(),
            "image changed size while reading"
        );
        Ok(bytes.into_boxed_slice())
    }
}

fn read_signature(reader: &mut BufReader<File>, bytes: &mut [u8]) -> std::io::Result<bool> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err),
    }
}
