//! Engine-owned still readers return premultiplied sRGB RGBA8 at the coded size.
use super::{
    gif,
    image_file::{ImageFile, ImageFormat},
    webp,
};
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) enum Still {
    WebP(webp::Still),
    Gif { size: [u32; 2], rgba: Vec<u8> },
}

impl Still {
    pub(crate) fn open(path: &Path) -> Result<Option<Self>> {
        let Some(source) = ImageFile::open(path)? else {
            return Ok(None);
        };
        let still = match source.format {
            ImageFormat::WebP => webp::Still::open(source).map(Self::WebP),
            ImageFormat::Gif => source
                .into_bytes()
                .and_then(gif::decode_still)
                .map(|(size, rgba)| Self::Gif { size, rgba }),
        };
        still
            .map(Some)
            .with_context(|| format!("open still image {}", path.display()))
    }

    pub(crate) fn size(&self) -> [u32; 2] {
        match self {
            Self::WebP(still) => still.size(),
            Self::Gif { size, .. } => *size,
        }
    }

    pub(crate) fn decode(self, path: &Path) -> Result<Vec<u8>> {
        match self {
            Self::WebP(still) => still.decode(path),
            Self::Gif { rgba, .. } => Ok(rgba),
        }
    }
}
