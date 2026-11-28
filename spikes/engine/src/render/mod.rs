pub(crate) mod affine;
pub(crate) mod channels;
pub(crate) mod curve;
pub(crate) mod gpu;
pub(crate) mod image;
pub(crate) mod path;
pub(crate) mod path_trace;
pub(crate) mod rect;
pub(crate) mod svg;
pub(crate) mod text;

use crate::graphics::{Content, Graphic, Mask, NativeRect};
use crate::zeroed_bytes as transparent_pixels;
use affine::{LayerSpace, SampledTransform};

pub(crate) struct LayerSample {
    pub frame: u32,
    pub opacity: f32,
    pub space: LayerSpace,
}

/// Evaluate the same local clock and spatial pose for drawing and picking.
pub(crate) fn sample_layer(
    layer: &Graphic,
    parent: LayerSpace,
    parent_frame: u32,
    fraction: f64,
) -> Option<LayerSample> {
    let frame = layer.local_frame(parent_frame)?;
    let motion = layer.motion_at(frame, fraction);
    let space = parent.child(
        layer.placed_position(),
        SampledTransform::new(layer.transform, motion),
    );
    Some(LayerSample {
        frame,
        opacity: motion.opacity,
        space,
    })
}

pub(crate) fn sample_mask(mask: &Mask, frame: u32, fraction: f64) -> Option<Content> {
    let rect = mask.animated_rect()?.sampled(frame, fraction);
    Some(Content::Rect(NativeRect::mask(rect)))
}

/// A small raster can travel far in local coordinates. Destination effects
/// use the swept bounds, while source effects allocate one pose at a time.
pub(crate) struct RasterEnvelope {
    pub swept: ([i32; 2], [u32; 2]),
    pub max_size: [u32; 2],
}

impl RasterEnvelope {
    fn fixed(bounds: ([i32; 2], [u32; 2])) -> Self {
        Self {
            swept: bounds,
            max_size: bounds.1,
        }
    }
}
