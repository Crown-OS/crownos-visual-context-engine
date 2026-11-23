//! How much of a frame looks like a photo, a video or a canvas rather than text. Text on a flat
//! background has few distinct grey levels in a tile; an image has many.

use crate::frame::CapturedFrame;
use crate::geometry::Rect;
use crate::tiles::{TILE_SIZE, TileGrid};
use crate::util::row_buffer::RowBuffer;

const SAMPLE_STEP: u32 = 4;
/// Of 64 grey levels, a tile with at least this many among its samples is image-like.
const IMAGE_LIKE_GREY_LEVELS: u32 = 20;

/// The share of tiles that look like an image, from 0 to 1.
pub(crate) fn image_like_share(frame: &CapturedFrame) -> f32 {
    let grid = TileGrid::for_size(frame.width, frame.height);
    let tile_count = grid.tile_count();
    if tile_count == 0 {
        return 0.0;
    }
    let mut rows = RowBuffer::default();
    let image_like = (0..tile_count)
        .filter(|index| is_image_like(frame, grid.tile_bounds(*index), &mut rows))
        .count();
    image_like as f32 / tile_count as f32
}

fn is_image_like(frame: &CapturedFrame, tile: Rect, rows: &mut RowBuffer) -> bool {
    if tile.width < TILE_SIZE / 2 || tile.height < TILE_SIZE / 2 {
        return false;
    }
    let mut grey_levels_seen = 0_u64;
    for y in (tile.y..tile.bottom()).step_by(SAMPLE_STEP as usize) {
        let Some(row) = rows.read(frame, tile.x, y, tile.width) else {
            continue;
        };
        for pixel in row
            .chunks_exact(CapturedFrame::BYTES_PER_PIXEL)
            .step_by(SAMPLE_STEP as usize)
        {
            if let [blue, green, red, _] = pixel {
                let grey = (u32::from(*blue) + 2 * u32::from(*green) + u32::from(*red)) >> 4;
                grey_levels_seen |= 1 << grey.min(63);
            }
        }
    }
    grey_levels_seen.count_ones() >= IMAGE_LIKE_GREY_LEVELS
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_frames::{frame_with, paint_rect};

    #[test]
    fn text_is_not_an_image_and_noise_is() {
        let text = frame_with(
            64,
            64,
            |pixels| paint_rect(pixels, 64, Rect::new(4, 4, 40, 8)),
            vec![],
        );
        assert!(image_like_share(&text) < 0.01);

        let photo = frame_with(
            64,
            64,
            |pixels| {
                for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                    let noise = (index as u64).wrapping_mul(2_654_435_761).to_le_bytes();
                    pixel[..3].copy_from_slice(&noise[1..4]);
                }
            },
            vec![],
        );
        assert!(image_like_share(&photo) > 0.9);
    }
}
