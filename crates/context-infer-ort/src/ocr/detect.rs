use std::collections::BTreeMap;

use context_infer::{Image, InferError, TextBounds};

use super::components::text_boxes;
use super::tensor::{BgrPlanes, run_single};
use crate::session::GraphSession;

const DEFAULT_TILE: usize = 640;
/// Text lines up to this height are whole in at least one tile.
const TILE_OVERLAP: usize = 64;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STANDARD_DEVIATION: [f32; 3] = [0.229, 0.224, 0.225];

/// The DB detector, run on tiles of the graph's static input size at native resolution, so
/// small UI text keeps its pixels.
#[derive(Debug)]
pub(super) struct TextDetector {
    graph: GraphSession,
    tile_width: usize,
    tile_height: usize,
}

impl TextDetector {
    pub(super) fn new(graph: GraphSession, shapes: &BTreeMap<String, Vec<i64>>) -> Self {
        let static_side = |index: usize| {
            shapes
                .values()
                .next()
                .and_then(|shape| shape.get(index))
                .and_then(|&side| usize::try_from(side).ok())
                .filter(|&side| side > TILE_OVERLAP)
                .unwrap_or(DEFAULT_TILE)
        };
        Self {
            tile_height: static_side(2),
            tile_width: static_side(3),
            graph,
        }
    }

    /// Text boxes in reading order.
    pub(super) fn detect(&self, image: &Image) -> Result<Vec<TextBounds>, InferError> {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let mut probability = vec![0u8; width * height];
        let input = self.graph.first_input()?;
        let output = self.graph.first_output()?;
        let mut values = vec![0f32; 3 * self.tile_width * self.tile_height];
        for top in tile_origins(height, self.tile_height) {
            for left in tile_origins(width, self.tile_width) {
                self.fill_tile(image, left, top, &mut values);
                let shape = [1, 3, self.tile_height, self.tile_width];
                let (_, tile) = run_single(&mut self.graph.lock(), input, output, shape, &values)?;
                merge_tile(
                    &mut probability,
                    width,
                    height,
                    left,
                    top,
                    self.tile_width,
                    &tile,
                );
            }
        }
        let mut boxes = text_boxes(&probability, width, height);
        sort_in_reading_order(&mut boxes);
        Ok(boxes)
    }

    fn fill_tile(&self, image: &Image, left: usize, top: usize, values: &mut [f32]) {
        values.fill(0.0);
        let Some((mut planes, _)) = BgrPlanes::split(values, self.tile_width * self.tile_height)
        else {
            return;
        };
        let format = image.format();
        for row in 0..self.tile_height {
            let Some(pixels) = u32::try_from(top + row).ok().and_then(|y| image.row(y)) else {
                break;
            };
            let pixels = pixels
                .chunks_exact(format.bytes_per_pixel())
                .skip(left)
                .take(self.tile_width)
                .map(|pixel| format.rgb(pixel));
            planes.write_row(row, self.tile_width, pixels, |value, channel| {
                let mean = MEAN.get(channel).copied().unwrap_or(0.5);
                let deviation = STANDARD_DEVIATION.get(channel).copied().unwrap_or(0.25);
                (f32::from(value) / 255.0 - mean) / deviation
            });
        }
    }
}

/// Tile starts that cover `length`, overlapping by [`TILE_OVERLAP`]; the last tile ends at
/// the edge.
fn tile_origins(length: usize, tile: usize) -> Vec<usize> {
    if length <= tile {
        return vec![0];
    }
    let step = tile - TILE_OVERLAP;
    let last = length - tile;
    let mut origins: Vec<usize> = (0..last).step_by(step).collect();
    origins.push(last);
    origins
}

/// Keeps the highest probability where tiles overlap, as 0-255.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn merge_tile(
    probability: &mut [u8],
    width: usize,
    height: usize,
    left: usize,
    top: usize,
    tile_width: usize,
    tile: &[f32],
) {
    let columns = tile_width.min(width.saturating_sub(left));
    for (row, values) in tile.chunks_exact(tile_width).enumerate() {
        let y = top + row;
        if y >= height {
            break;
        }
        let start = y * width + left;
        let Some(target) = probability.get_mut(start..start + columns) else {
            continue;
        };
        for (merged, &value) in target.iter_mut().zip(values) {
            *merged = (*merged).max((value.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
}

/// Top to bottom; boxes whose centres are within half a line of each other count as one
/// line and go left to right.
fn sort_in_reading_order(boxes: &mut [TextBounds]) {
    boxes.sort_by_key(|bounds| (bounds.y + bounds.height / 2, bounds.x));
    let mut index = 1;
    while index < boxes.len() {
        if let (Some(previous), Some(current)) = (boxes.get(index - 1), boxes.get(index)) {
            let previous_centre = previous.y + previous.height / 2;
            let current_centre = current.y + current.height / 2;
            let same_line =
                current_centre.abs_diff(previous_centre) < previous.height.min(current.height) / 2;
            if same_line && current.x < previous.x {
                boxes.swap(index - 1, index);
                index = index.saturating_sub(1).max(1);
                continue;
            }
        }
        index += 1;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_image_with_an_overlap() {
        assert_eq!(tile_origins(500, 640), [0]);
        assert_eq!(tile_origins(1440, 640), [0, 576, 800]);
        assert_eq!(tile_origins(640, 640), [0]);
    }

    #[test]
    fn overlapping_tiles_keep_the_highest_probability() {
        let mut probability = vec![0u8; 4 * 2];
        merge_tile(
            &mut probability,
            4,
            2,
            0,
            0,
            3,
            &[1.0, 0.5, 0.0, 0.0, 0.0, 0.0],
        );
        merge_tile(
            &mut probability,
            4,
            2,
            2,
            0,
            3,
            &[1.0, 1.0, 1.0, 0.2, 0.2, 0.2],
        );
        assert_eq!(probability, [255, 127, 255, 255, 0, 0, 51, 51]);
    }

    #[test]
    fn words_on_one_line_read_left_to_right() {
        let word = |x, y| TextBounds {
            x,
            y,
            width: 30,
            height: 12,
        };
        let mut boxes = vec![word(10, 40), word(10, 11), word(100, 10)];
        sort_in_reading_order(&mut boxes);
        assert_eq!(boxes, [word(10, 11), word(100, 10), word(10, 40)]);
    }
}
