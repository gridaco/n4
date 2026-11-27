use crate::{MAX_SURFACE_DIMENSION, graphics::Raster};
use anyhow::{Context, Result, ensure};
use skia_safe::{AlphaType, ColorSpace, ColorType, Data, Image, ImageInfo, image::CachingHint};
use std::path::Path;

fn open(path: &Path) -> Result<Image> {
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
    let im = open(path)?;
    Ok([im.width() as u32, im.height() as u32])
}

pub(crate) fn paint_image(path: &Path) -> Result<Raster> {
    let im = open(path)?;
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
