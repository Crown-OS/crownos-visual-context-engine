use std::io::Cursor;

use context_infer::{Image, InferError};

/// A PNG of `image`; the only place where pixels are encoded, because they leave the process.
pub(crate) fn encode_png(image: &Image) -> Result<Vec<u8>, InferError> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(Cursor::new(&mut png), image.width(), image.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let failed =
        |error: png::EncodingError| InferError::Backend(format!("encoding a PNG: {error}"));
    let mut writer = encoder.write_header().map_err(failed)?;
    writer.write_image_data(&image.to_rgb8()).map_err(failed)?;
    writer.finish().map_err(failed)?;
    Ok(png)
}
