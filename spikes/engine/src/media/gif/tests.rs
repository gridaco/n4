use super::*;
use gif::{Encoder, Frame};
use std::borrow::Cow;

// Original test patterns, generated in memory. Palette index 0 = red, 1 = blue.
const PALETTE: &[u8] = &[255, 0, 0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0; 4];

fn patch(x: u16, indices: &'static [u8]) -> Frame<'static> {
    Frame {
        left: x,
        width: indices.len() as u16,
        height: 1,
        buffer: Cow::Borrowed(indices),
        ..Default::default()
    }
}

fn gif(width: u16, height: u16, frames: &[Frame<'_>]) -> Result<Vec<u8>> {
    let mut encoder = Encoder::new(Vec::new(), width, height, PALETTE)?;
    encoder.set_repeat(Repeat::Finite(3))?;
    for frame in frames {
        encoder.write_frame(frame)?;
    }
    Ok(encoder.into_inner()?)
}

#[test]
fn interlaced_local_palette_uses_display_row_order() -> Result<()> {
    // GIF stores eight rows in this pass order: 0, 4, 2, 6, 1, 3, 5, 7.
    let local: Vec<u8> = (0..8).flat_map(|i| [i * 30, 0, 0]).collect();
    let frame = Frame {
        width: 1,
        height: 8,
        interlaced: true,
        palette: Some(local),
        buffer: Cow::Borrowed(&[0, 4, 2, 6, 1, 3, 5, 7]),
        ..Default::default()
    };
    let bytes = gif(1, 8, &[frame])?;
    let (size, rgba) = decode_still(bytes.clone().into())?;
    assert_eq!(size, [1, 8]);
    assert_eq!(
        rgba,
        (0..8).flat_map(|i| [i * 30, 0, 0, 255]).collect::<Vec<_>>()
    );
    let error = Animation::parse(bytes.into())
        .err()
        .expect("loop extension alone is still");
    assert!(error.to_string().contains("<image>"));
    Ok(())
}

#[test]
fn missing_and_zero_delays_have_deterministic_holds() -> Result<()> {
    let first = patch(0, &[1]); // remove its control extension to represent a missing delay
    let second = patch(1, &[1]); // explicit zero delay
    let third = Frame {
        delay: 1,
        ..patch(0, &[1])
    };
    let mut bytes = gif(2, 1, &[first, second, third])?;
    let control = bytes.windows(3).position(|b| b == [0x21, 0xf9, 4]).unwrap();
    bytes.drain(control..control + 8);
    let animation = Animation::parse(bytes.clone().into())?;
    assert_eq!(animation.durations(), &[100, 100, 10]);
    assert_eq!(animation.repeat, Repeat::Finite(3));
    assert!(
        decode_still(bytes.into())
            .unwrap_err()
            .to_string()
            .contains("<video>")
    );
    Ok(())
}

#[test]
fn clears_to_valid_global_background_or_transparency() -> Result<()> {
    let bytes = gif(
        3,
        1,
        &[
            Frame {
                dispose: DisposalMethod::Background,
                ..patch(1, &[1])
            },
            patch(2, &[1]),
        ],
    )?;
    let mut cursor = Animation::parse(bytes.clone().into())?.new_cursor()?;
    assert_eq!(cursor.canvas(1)?.as_ref(), [RED, RED, BLUE].as_flattened());
    assert_eq!(cursor.canvas(0)?.as_ref(), [RED, BLUE, RED].as_flattened());

    let mut invalid_bg = bytes;
    invalid_bg[11] = 7; // no global color at that index
    let mut cursor = Animation::parse(invalid_bg.into())?.new_cursor()?;
    assert_eq!(
        cursor.canvas(1)?.as_ref(),
        [CLEAR, CLEAR, BLUE].as_flattened()
    );
    Ok(())
}

#[test]
fn rejects_input_wait_bad_indices_and_long_or_short_image_data() -> Result<()> {
    for (bad, reason) in [
        (
            Frame {
                needs_user_input: true,
                ..patch(0, &[0])
            },
            "user input",
        ),
        (
            Frame {
                transparent: Some(5),
                ..patch(0, &[0])
            },
            "transparent palette index",
        ),
        (patch(0, &[5]), "pixel palette index"),
        (
            Frame {
                width: 1,
                ..patch(0, &[0, 1])
            },
            "exceeds its rectangle",
        ),
        (patch(2, &[0]), "out-of-bounds"),
    ] {
        let bytes = gif(2, 1, &[patch(0, &[0]), bad])?;
        let error = Animation::parse(bytes.into())
            .err()
            .expect("damaged final frame accepted");
        assert!(
            format!("{error:#}").contains(reason),
            "{error:#} (expected {reason})"
        );
    }
    let mut short = gif(2, 1, &[patch(0, &[0])])?;
    let at = short.iter().rposition(|&b| b == 0x2c).unwrap();
    short[at + 5] = 2; // claims two pixels with one LZW index
    assert!(decode_still(short.into()).is_err());
    Ok(())
}

#[test]
fn rejects_oversized_canvas_or_partial_or_concatenated_stream() -> Result<()> {
    let bytes = gif(2, 1, &[patch(0, &[0]), patch(1, &[0])])?;
    let mut oversized = bytes.clone();
    oversized[6..8].copy_from_slice(&8193u16.to_le_bytes());
    assert!(
        Animation::parse(oversized.into())
            .err()
            .unwrap()
            .to_string()
            .contains("dimensions")
    );
    assert!(Animation::parse(bytes[..bytes.len() - 1].into()).is_err());
    let mut extra = bytes;
    extra.push(0x3b);
    assert!(
        Animation::parse(extra.into())
            .err()
            .unwrap()
            .to_string()
            .contains("trailer")
    );
    Ok(())
}

#[test]
fn gif87a_still_without_graphics_control_is_opaque() -> Result<()> {
    let mut bytes = gif(2, 1, &[patch(0, &[0, 1])])?;
    bytes[..6].copy_from_slice(b"GIF87a");
    // Remove the GIF89a animation and graphic control extensions.
    let first_extension = 13 + PALETTE.len();
    let image = bytes.iter().position(|&b| b == 0x2c).unwrap();
    bytes.drain(first_extension..image);
    assert_eq!(decode_still(bytes.into())?.1, [RED, BLUE].as_flattened());
    Ok(())
}
