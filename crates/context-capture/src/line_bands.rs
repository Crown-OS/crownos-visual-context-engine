//! Splits a crop into horizontal bands of text lines at the rows that are one flat colour, and
//! gives each band a hash of its grayscale content. The hash ignores where the band is, so a
//! line that scrolled keeps its hash.

use xxhash_rust::xxh3::Xxh3;
use zeroize::Zeroize;

use crate::frame::CapturedFrame;
use crate::geometry::Rect;
use crate::util::hash_seed::hash_seed;
use crate::util::row_buffer::RowBuffer;

/// Grayscale pixels go to the hasher through this small stack buffer, which is zeroed after.
const LUMA_CHUNK: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineBand {
    pub bounds: Rect,
    pub hash: u64,
}

/// The bands of `crop`, top to bottom. A crop without flat rows is one band.
pub(crate) fn find_line_bands(frame: &CapturedFrame, crop: Rect) -> Vec<LineBand> {
    let mut bands = Vec::new();
    let mut band_top = None;
    let mut rows = RowBuffer::default();
    for y in crop.y..crop.bottom() {
        let flat = rows
            .read(frame, crop.x, y, crop.width)
            .is_none_or(is_flat_row);
        match (flat, band_top) {
            (false, None) => band_top = Some(y),
            (true, Some(top)) => {
                bands.extend(band(
                    frame,
                    Rect::new(crop.x, top, crop.width, y - top),
                    &mut rows,
                ));
                band_top = None;
            }
            _ => {}
        }
    }
    if let Some(top) = band_top {
        bands.extend(band(
            frame,
            Rect::new(crop.x, top, crop.width, crop.bottom() - top),
            &mut rows,
        ));
    }
    bands
}

fn is_flat_row(row: &[u8]) -> bool {
    let mut pixels = row.chunks_exact(CapturedFrame::BYTES_PER_PIXEL);
    let Some(first) = pixels.next() else {
        return true;
    };
    pixels.all(|pixel| same_colour(pixel, first))
}

fn same_colour(pixel: &[u8], other: &[u8]) -> bool {
    pixel.get(..3) == other.get(..3)
}

/// The band trimmed to the columns that differ from its background, and hashed.
fn band(frame: &CapturedFrame, rows: Rect, buffer: &mut RowBuffer) -> Option<LineBand> {
    let background: [u8; CapturedFrame::BYTES_PER_PIXEL] =
        buffer.read(frame, rows.x, rows.y, 1)?.try_into().ok()?;
    let mut left = rows.right();
    let mut right = rows.x;
    for y in rows.y..rows.bottom() {
        let row = buffer.read(frame, rows.x, y, rows.width)?;
        let differs = |(_, pixel): &(usize, &[u8])| !same_colour(pixel, &background);
        let mut columns = row
            .chunks_exact(CapturedFrame::BYTES_PER_PIXEL)
            .enumerate()
            .filter(differs);
        if let Some((first, _)) = columns.next() {
            let last = columns.next_back().map_or(first, |(last, _)| last);
            left = left.min(rows.x + u32::try_from(first).ok()?);
            right = right.max(rows.x + u32::try_from(last).ok()? + 1);
        }
    }
    let bounds = if right > left {
        Rect::new(left, rows.y, right - left, rows.height)
    } else {
        rows
    };
    Some(LineBand {
        bounds,
        hash: grayscale_hash(frame, bounds, buffer),
    })
}

fn grayscale_hash(frame: &CapturedFrame, bounds: Rect, rows: &mut RowBuffer) -> u64 {
    let mut hasher = Xxh3::with_seed(hash_seed());
    hasher.update(&bounds.width.to_le_bytes());
    hasher.update(&bounds.height.to_le_bytes());
    let mut luma_chunk = [0_u8; LUMA_CHUNK];
    for y in bounds.y..bounds.bottom() {
        let Some(row) = rows.read(frame, bounds.x, y, bounds.width) else {
            continue;
        };
        for pixels in row.chunks(LUMA_CHUNK * CapturedFrame::BYTES_PER_PIXEL) {
            let mut filled = 0;
            for (slot, pixel) in luma_chunk
                .iter_mut()
                .zip(pixels.chunks_exact(CapturedFrame::BYTES_PER_PIXEL))
            {
                *slot = luma(pixel);
                filled += 1;
            }
            hasher.update(luma_chunk.get(..filled).unwrap_or_default());
        }
    }
    luma_chunk.zeroize();
    hasher.digest()
}

/// Rec. 601 luma of a B, G, R, X pixel.
fn luma(pixel: &[u8]) -> u8 {
    match pixel {
        [blue, green, red, ..] => {
            let weighted = 29 * u32::from(*blue) + 150 * u32::from(*green) + 77 * u32::from(*red);
            u8::try_from(weighted >> 8).unwrap_or(u8::MAX)
        }
        _ => 0,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_frames::{frame_with, paint_rect};

    #[test]
    fn flat_rows_split_the_crop_into_trimmed_bands() {
        let frame = frame_with(
            100,
            60,
            |pixels| {
                paint_rect(pixels, 100, Rect::new(10, 5, 30, 8));
                paint_rect(pixels, 100, Rect::new(20, 30, 50, 10));
            },
            vec![],
        );
        let bands = find_line_bands(&frame, frame.bounds());
        let bounds: Vec<Rect> = bands.iter().map(|band| band.bounds).collect();
        assert_eq!(bounds, [Rect::new(10, 5, 30, 8), Rect::new(20, 30, 50, 10)]);
    }

    #[test]
    fn a_moved_line_keeps_its_hash() {
        let line = |x: u32, y: u32| {
            frame_with(
                120,
                80,
                move |pixels| {
                    paint_rect(pixels, 120, Rect::new(x, y, 30, 8));
                    paint_rect(pixels, 120, Rect::new(x + 4, y + 2, 6, 4));
                },
                vec![],
            )
        };
        let first = line(10, 10);
        let scrolled = line(40, 50);
        let hash_of = |frame: &CapturedFrame| find_line_bands(frame, frame.bounds())[0].hash;
        assert_eq!(hash_of(&first), hash_of(&scrolled));
    }
}
