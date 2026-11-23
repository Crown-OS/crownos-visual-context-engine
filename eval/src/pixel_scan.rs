//! Finds pixels where they must not be. A test window draws flat blocks of seeded colours. A
//! capture keeps a flat block flat when the compositor scales it, so a run of
//! [`REPEATS`] pixels of one block colour, in any 3- or 4-byte pixel layout and in RGB or BGR
//! order, means a frame was written. Such a run of a seeded colour does not appear by chance.

use std::collections::HashSet;

use crate::util::rng::Rng;

/// The side of one flat block, in pixels.
pub const BLOCK: u32 = 32;
/// Blocks per row and column that a window of up to 2048 x 2048 pixels can show.
const MAX_BLOCKS: u32 = 64;
/// How many equal pixels in a row make a find.
const REPEATS: usize = 8;
const BYTES_PER_PIXEL: usize = 4;

/// The colour of block (`column`, `row`) for `seed`, as R, G, B. Each channel is in 40..=215 and
/// the channels differ, so no grey, black or white of a user interface matches it.
fn block_colour(seed: u64, column: u32, row: u32) -> [u8; 3] {
    let block = u64::from(column) << 32 | u64::from(row);
    let mut rng = Rng::seeded(
        seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ block.wrapping_mul(0xC2B2_AE3D_27D4_EB4F),
    );
    loop {
        let [red, green, blue, ..] = rng.next_u64().to_le_bytes();
        let colour = [red, green, blue].map(|channel| 40 + channel % 176);
        if colour[0] != colour[1] && colour[1] != colour[2] && colour[0] != colour[2] {
            return colour;
        }
    }
}

/// `width * height` pixels of flat seeded blocks in `XRGB8888` memory order (B, G, R, X).
pub fn colour_blocks(seed: u64, width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
    for y in 0..height {
        for x in 0..width {
            let [red, green, blue] = block_colour(seed, x / BLOCK, y / BLOCK);
            pixels.extend_from_slice(&[blue, green, red, 0xFF]);
        }
    }
    pixels
}

/// The block colours of some seeds.
#[derive(Debug, Default)]
pub struct PixelNeedles {
    colours: HashSet<[u8; 3]>,
}

impl PixelNeedles {
    /// Adds every block colour that [`colour_blocks`] can draw for `seed`.
    pub fn add_seed(&mut self, seed: u64) {
        for column in 0..MAX_BLOCKS {
            for row in 0..MAX_BLOCKS {
                self.colours.insert(block_colour(seed, column, row));
            }
        }
    }

    pub fn len(&self) -> usize {
        self.colours.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colours.is_empty()
    }

    /// Whether `haystack` holds [`REPEATS`] equal pixels of a block colour: 3 or 4 bytes per
    /// pixel at any offset, R, G, B or B, G, R first, the fourth byte ignored.
    pub fn found_in(&self, haystack: &[u8]) -> bool {
        [3, 4].into_iter().any(|pixel_size| {
            (0..pixel_size).any(|offset| self.found_with(haystack, pixel_size, offset))
        })
    }

    fn found_with(&self, haystack: &[u8], pixel_size: usize, offset: usize) -> bool {
        let mut previous: Option<[u8; 3]> = None;
        let mut run = 0;
        for pixel in haystack
            .get(offset..)
            .unwrap_or_default()
            .chunks_exact(pixel_size)
        {
            let Some(&[first, second, third]) = pixel.get(..3) else {
                return false;
            };
            let colour = [first, second, third];
            run = if previous == Some(colour) { run + 1 } else { 1 };
            previous = Some(colour);
            if run >= REPEATS
                && (self.colours.contains(&colour)
                    || self.colours.contains(&[third, second, first]))
            {
                return true;
            }
        }
        false
    }
}

/// The image format whose signature `bytes` holds at its start, if any.
pub fn image_signature(bytes: &[u8]) -> Option<&'static str> {
    const SIGNATURES: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "PNG"),
        (b"\xFF\xD8\xFF", "JPEG"),
        (b"GIF87a", "GIF"),
        (b"GIF89a", "GIF"),
        (b"qoif", "QOI"),
        (b"II*\0", "TIFF"),
        (b"MM\0*", "TIFF"),
        (b"P6\n", "PPM"),
    ];
    if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("WebP");
    }
    SIGNATURES
        .iter()
        .find(|(signature, _)| bytes.starts_with(signature))
        .map(|(_, format)| *format)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn needles(seed: u64) -> PixelNeedles {
        let mut needles = PixelNeedles::default();
        needles.add_seed(seed);
        needles
    }

    #[test]
    fn a_block_is_found_as_it_is_packed_and_scaled() {
        let pixels = colour_blocks(7, 96, 64);
        let needles = needles(7);

        let mut file = b"header".to_vec();
        file.extend_from_slice(&pixels[401..460]);
        assert!(needles.found_in(&file));

        let rgb: Vec<u8> = pixels
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            .collect();
        assert!(needles.found_in(&rgb[1001..1040]));

        let doubled: Vec<u8> = pixels
            .chunks_exact(4)
            .take(96)
            .flat_map(|pixel| [pixel, pixel].concat())
            .collect();
        assert!(needles.found_in(&doubled));
    }

    #[test]
    fn other_seeds_short_runs_and_text_are_not_found() {
        let needles = needles(7);
        assert!(!needles.found_in(&colour_blocks(8, 96, 64)));
        assert!(!needles.found_in(&colour_blocks(7, 96, 64)[..4 * (REPEATS - 1)]));
        assert!(!needles.found_in(&[b' '; 4096]));
        assert!(!needles.found_in(&[0; 4096]));
    }

    #[test]
    fn image_signatures_are_named() {
        assert_eq!(image_signature(b"\x89PNG\r\n\x1a\nrest"), Some("PNG"));
        assert_eq!(image_signature(b"RIFF\0\0\0\0WEBPVP8 "), Some("WebP"));
        assert_eq!(image_signature(b"plain text"), None);
    }
}
