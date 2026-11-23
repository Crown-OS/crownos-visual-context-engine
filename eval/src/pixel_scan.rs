//! Finds pixels where they must not be. A test window draws seeded noise; any 16-byte run of
//! that noise, in the compositor's B, G, R, X order or packed as RGB or BGR, cannot appear by
//! chance, so finding one in a file or a database value means a frame was written.

use std::collections::HashSet;

use crate::util::rng::Rng;

const NEEDLE_LENGTH: usize = 16;
const BYTES_PER_PIXEL: usize = 4;

/// `width * height` pixels of seeded noise in `XRGB8888` memory order, with X = 0xFF.
pub fn noise_pixels(seed: u64, width: u32, height: u32) -> Vec<u8> {
    let mut rng = Rng::seeded(seed);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
    for _ in 0..width as usize * height as usize {
        let [blue, green, red, ..] = rng.next_u64().to_le_bytes();
        pixels.extend_from_slice(&[blue, green, red, 0xFF]);
    }
    pixels
}

/// Every pixel-aligned 16-byte run of some frames, in each byte order a copy could use.
#[derive(Debug, Default)]
pub struct PixelNeedles {
    needles: HashSet<[u8; NEEDLE_LENGTH]>,
}

impl PixelNeedles {
    /// Adds the runs of `XRGB8888` pixels as they are, as packed RGB and as packed BGR.
    pub fn add_xrgb(&mut self, pixels: &[u8]) {
        let mut rgb = Vec::with_capacity(pixels.len() / BYTES_PER_PIXEL * 3);
        let mut bgr = Vec::with_capacity(rgb.capacity());
        for pixel in pixels.chunks_exact(BYTES_PER_PIXEL) {
            if let [blue, green, red, _] = pixel {
                rgb.extend_from_slice(&[*red, *green, *blue]);
                bgr.extend_from_slice(&[*blue, *green, *red]);
            }
        }
        self.add_runs(pixels, BYTES_PER_PIXEL);
        self.add_runs(&rgb, 3);
        self.add_runs(&bgr, 3);
    }

    pub fn len(&self) -> usize {
        self.needles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    /// Whether `haystack` holds a run of the frames at any offset.
    pub fn found_in(&self, haystack: &[u8]) -> bool {
        haystack.windows(NEEDLE_LENGTH).any(|window| {
            <[u8; NEEDLE_LENGTH]>::try_from(window).is_ok_and(|run| self.needles.contains(&run))
        })
    }

    fn add_runs(&mut self, bytes: &[u8], step: usize) {
        let mut start = 0;
        while let Some(window) = bytes.get(start..start + NEEDLE_LENGTH) {
            if let Ok(run) = <[u8; NEEDLE_LENGTH]>::try_from(window) {
                self.needles.insert(run);
            }
            start += step;
        }
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

    #[test]
    fn a_copied_run_is_found_in_every_byte_order_and_noise_is_not() {
        let pixels = noise_pixels(7, 64, 32);
        let mut needles = PixelNeedles::default();
        needles.add_xrgb(&pixels);

        let mut file = b"header".to_vec();
        file.extend_from_slice(&pixels[401..460]);
        assert!(needles.found_in(&file));

        let rgb: Vec<u8> = pixels
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            .collect();
        assert!(needles.found_in(&rgb[1000..1040]));

        let other = noise_pixels(8, 64, 32);
        assert!(!needles.found_in(&other));
        assert!(!needles.found_in(&pixels[..15]));
    }

    #[test]
    fn image_signatures_are_named() {
        assert_eq!(image_signature(b"\x89PNG\r\n\x1a\nrest"), Some("PNG"));
        assert_eq!(image_signature(b"RIFF\0\0\0\0WEBPVP8 "), Some("WebP"));
        assert_eq!(image_signature(b"plain text"), None);
    }
}
