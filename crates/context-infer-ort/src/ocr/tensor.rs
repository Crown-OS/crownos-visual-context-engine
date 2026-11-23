use context_infer::{Image, InferError};
use ort::session::Session;
use ort::value::TensorRef;

use crate::util::error::backend_error;

/// Planes of one NCHW image in PP-OCR order: blue, green, red.
pub(super) struct BgrPlanes<'a> {
    pub(super) blue: &'a mut [f32],
    pub(super) green: &'a mut [f32],
    pub(super) red: &'a mut [f32],
}

impl<'a> BgrPlanes<'a> {
    /// Splits one image of `plane` values per channel off the front of `values`.
    pub(super) const fn split(
        values: &'a mut [f32],
        plane: usize,
    ) -> Option<(Self, &'a mut [f32])> {
        if values.len() < plane * 3 {
            return None;
        }
        let (blue, rest) = values.split_at_mut(plane);
        let (green, rest) = rest.split_at_mut(plane);
        let (red, rest) = rest.split_at_mut(plane);
        Some((Self { blue, green, red }, rest))
    }

    /// Writes row `row` from `pixels`, normalised per channel as `normalize(value, channel)`
    /// with channel 0 blue, 1 green, 2 red.
    pub(super) fn write_row(
        &mut self,
        row: usize,
        width: usize,
        pixels: impl Iterator<Item = [u8; 3]>,
        normalize: impl Fn(u8, usize) -> f32,
    ) {
        let range = row * width..(row + 1) * width;
        let (Some(blue), Some(green), Some(red)) = (
            self.blue.get_mut(range.clone()),
            self.green.get_mut(range.clone()),
            self.red.get_mut(range),
        ) else {
            return;
        };
        for (((blue, green), red), [r, g, b]) in blue
            .iter_mut()
            .zip(green.iter_mut())
            .zip(red.iter_mut())
            .zip(pixels)
        {
            *blue = normalize(b, 0);
            *green = normalize(g, 1);
            *red = normalize(r, 2);
        }
    }
}

/// Runs `session` on one f32 tensor and gives its first output as shape and values.
pub(super) fn run_single(
    session: &mut Session,
    input: &str,
    output: &str,
    shape: [usize; 4],
    values: &[f32],
) -> Result<(Vec<i64>, Vec<f32>), InferError> {
    let tensor = TensorRef::from_array_view((shape, values))
        .map_err(|error| backend_error("building the input tensor", error))?;
    let outputs = session
        .run(ort::inputs![input => tensor])
        .map_err(|error| backend_error("running the graph", error))?;
    let value = outputs
        .get(output)
        .ok_or_else(|| InferError::Backend(format!("no output {output}")))?;
    let (shape, values) = value
        .try_extract_tensor::<f32>()
        .map_err(|error| backend_error("reading the output", error))?;
    Ok((shape.to_vec(), values.to_vec()))
}

/// The red, green and blue at a fractional position, by bilinear interpolation.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn sample_bilinear(image: &Image, x: f32, y: f32) -> [u8; 3] {
    let left = x.floor().max(0.0);
    let top = y.floor().max(0.0);
    let (x_weight, y_weight) = ((x - left).clamp(0.0, 1.0), (y - top).clamp(0.0, 1.0));
    let (left, top) = (left as u32, top as u32);
    let right = (left + 1).min(image.width().saturating_sub(1));
    let bottom = (top + 1).min(image.height().saturating_sub(1));
    let corners = [
        image.rgb_at(left, top),
        image.rgb_at(right, top),
        image.rgb_at(left, bottom),
        image.rgb_at(right, bottom),
    ];
    let mut mixed = [0u8; 3];
    for (channel, value) in mixed.iter_mut().enumerate() {
        let corner = |index: usize| {
            corners
                .get(index)
                .and_then(|pixel| pixel.get(channel))
                .map_or(0.0, |&value| f32::from(value))
        };
        let upper = corner(0) * (1.0 - x_weight) + corner(1) * x_weight;
        let lower = corner(2) * (1.0 - x_weight) + corner(3) * x_weight;
        *value = (upper * (1.0 - y_weight) + lower * y_weight).round() as u8;
    }
    mixed
}
