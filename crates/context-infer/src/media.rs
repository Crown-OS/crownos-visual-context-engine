use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

/// Read-only bytes shared without a copy: a `Vec`, or a read-only memfd mapping.
#[derive(Clone)]
pub struct SharedBytes(Arc<dyn AsRef<[u8]> + Send + Sync>);

impl SharedBytes {
    pub fn new(bytes: impl AsRef<[u8]> + Send + Sync + 'static) -> Self {
        Self(Arc::new(bytes))
    }

    pub fn as_slice(&self) -> &[u8] {
        self.0.as_ref().as_ref()
    }
}

impl From<Vec<u8>> for SharedBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl fmt::Debug for SharedBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SharedBytes({} bytes)", self.as_slice().len())
    }
}

/// Packed 8-bit RGB rows, `stride` bytes apart.
#[derive(Clone, Debug)]
pub struct RgbImage {
    width: u32,
    height: u32,
    stride: usize,
    pixels: SharedBytes,
}

#[derive(Debug, thiserror::Error)]
#[error("a {width}x{height} RGB image with a stride of {stride} does not fit in {length} bytes")]
pub struct ImageLayoutError {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub length: usize,
}

impl RgbImage {
    pub const BYTES_PER_PIXEL: usize = 3;

    pub fn new(
        width: u32,
        height: u32,
        stride: usize,
        pixels: SharedBytes,
    ) -> Result<Self, ImageLayoutError> {
        let row_bytes = width as usize * Self::BYTES_PER_PIXEL;
        let needed = (height as usize)
            .checked_sub(1)
            .map_or(Some(0), |full_rows| {
                full_rows.checked_mul(stride)?.checked_add(row_bytes)
            });
        let length = pixels.as_slice().len();
        match needed {
            Some(needed) if stride >= row_bytes && needed <= length => Ok(Self {
                width,
                height,
                stride,
                pixels,
            }),
            _ => Err(ImageLayoutError {
                width,
                height,
                stride,
                length,
            }),
        }
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn stride(&self) -> usize {
        self.stride
    }

    pub fn pixels(&self) -> &[u8] {
        self.pixels.as_slice()
    }

    /// Each row without its stride padding, top to bottom.
    pub fn rows(&self) -> impl Iterator<Item = &[u8]> {
        let row_bytes = self.width as usize * Self::BYTES_PER_PIXEL;
        self.pixels()
            .chunks(self.stride.max(1))
            .take(self.height as usize)
            .filter_map(move |row| row.get(..row_bytes))
    }
}

/// Input that is not text. Local backends take raw pixels; only remote ones encode.
#[derive(Clone, Debug)]
pub enum Media {
    Rgb8(RgbImage),
    Encoded {
        mime: Cow<'static, str>,
        bytes: SharedBytes,
    },
    /// Mono 16 kHz signed 16-bit samples.
    Pcm16k(Arc<[i16]>),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn rows_skip_the_stride_padding() {
        let pixels: Vec<u8> = (0..16).collect();
        let image = RgbImage::new(2, 2, 8, pixels.into()).unwrap();
        let rows: Vec<&[u8]> = image.rows().collect();
        assert_eq!(rows, [&[0, 1, 2, 3, 4, 5][..], &[8, 9, 10, 11, 12, 13][..]]);
    }

    #[test]
    fn the_last_row_needs_no_padding() {
        assert!(RgbImage::new(2, 2, 8, vec![0; 14].into()).is_ok());
        assert!(RgbImage::new(2, 2, 8, vec![0; 13].into()).is_err());
    }

    #[test]
    fn a_stride_shorter_than_a_row_is_refused() {
        assert!(RgbImage::new(4, 1, 8, vec![0; 64].into()).is_err());
    }
}
