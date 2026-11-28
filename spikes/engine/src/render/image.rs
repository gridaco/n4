use crate::{MAX_SURFACE_DIMENSION, graphics::Raster, media::still::Still};
use anyhow::{Context, Result, ensure};
use skia_safe::{AlphaType, ColorSpace, ColorType, Data, Image, ImageInfo, image::CachingHint};
use std::{path::Path, sync::Arc};

#[cfg(test)]
mod tests;

fn open_skia(path: &Path) -> Result<Image> {
    let data =
        Data::from_filename(path).with_context(|| format!("read image {}", path.display()))?;
    let image =
        Image::from_encoded(data).with_context(|| format!("decode image {}", path.display()))?;
    ensure!(
        [image.width(), image.height()]
            .iter()
            .all(|&n| (1..=MAX_SURFACE_DIMENSION as i32).contains(&n)),
        "invalid image dimensions"
    );
    Ok(image)
}

pub(crate) fn dimensions(path: &Path) -> Result<[u32; 2]> {
    if let Some(still) = Still::open(path)? {
        return Ok(still.size());
    }
    skia_dimensions(path)
}

fn skia_dimensions(path: &Path) -> Result<[u32; 2]> {
    let im = open_skia(path)?;
    Ok([im.width() as u32, im.height() as u32])
}

fn read_still_raster(path: &Path) -> Result<Option<Raster>> {
    let Some(still) = Still::open(path)? else {
        return Ok(None);
    };
    Ok(Some(Raster {
        origin: [0; 2],
        size: still.size(),
        rgba: still.decode(path)?,
    }))
}

pub(crate) fn prepare_pinned(path: &Path) -> Result<Option<Arc<Raster>>> {
    Ok(read_still_raster(path)?.map(Arc::new))
}

#[cfg(any(test, feature = "host-audio"))]
pub(crate) enum PreparedImage {
    Pinned(Arc<Raster>),
    IntrinsicSize([u32; 2]),
}

#[cfg(any(test, feature = "host-audio"))]
pub(crate) fn prepare_image(path: &Path) -> Result<PreparedImage> {
    match prepare_pinned(path)? {
        Some(raster) => Ok(PreparedImage::Pinned(raster)),
        None => Ok(PreparedImage::IntrinsicSize(skia_dimensions(path)?)),
    }
}

pub(crate) fn paint_image(path: &Path) -> Result<Raster> {
    if let Some(raster) = read_still_raster(path)? {
        return Ok(raster);
    }
    let im = open_skia(path)?;
    let info = ImageInfo::new(
        im.dimensions(),
        ColorType::RGBA8888,
        AlphaType::Premul,
        ColorSpace::new_srgb(),
    );
    let stride = info.min_row_bytes();
    let mut rgba = super::transparent_pixels(stride * im.height() as usize)?;
    ensure!(
        im.read_pixels(&info, &mut rgba, stride, (0, 0), CachingHint::Disallow),
        "read image pixels {}",
        path.display()
    );
    Ok(Raster {
        origin: [0; 2],
        size: [im.width() as u32, im.height() as u32],
        rgba,
    })
}
