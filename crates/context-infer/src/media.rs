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

/// The byte order of one pixel in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// R, G, B.
    Rgb8,
    /// B, G, R, X: `wl_shm` and DRM `XRGB8888` on a little-endian machine.
    Xrgb8888,
    /// R, G, B, X: `wl_shm` and DRM `XBGR8888` on a little-endian machine.
    Xbgr8888,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Xrgb8888 | Self::Xbgr8888 => 4,
        }
    }

    /// The red, green and blue of one pixel of this format.
    pub fn rgb(self, pixel: &[u8]) -> [u8; 3] {
        match (self, pixel) {
            (Self::Rgb8 | Self::Xbgr8888, [red, green, blue, ..])
            | (Self::Xrgb8888, [blue, green, red, ..]) => [*red, *green, *blue],
            _ => [0; 3],
        }
    }
}

/// 8-bit pixel rows, `stride` bytes apart, as the compositor or a decoder gives them.
/// Backends convert the format in their own preprocessing, so no caller copies a frame.
#[derive(Clone, Debug)]
pub struct Image {
    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
    pixels: SharedBytes,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "a {width}x{height} {format:?} image with a stride of {stride} does not fit in {length} bytes"
)]
pub struct ImageLayoutError {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub length: usize,
}

impl Image {
    pub fn new(
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
        pixels: SharedBytes,
    ) -> Result<Self, ImageLayoutError> {
        let row_bytes = width as usize * format.bytes_per_pixel();
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
                format,
                pixels,
            }),
            _ => Err(ImageLayoutError {
                width,
                height,
                stride,
                format,
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

    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn pixels(&self) -> &[u8] {
        self.pixels.as_slice()
    }

    /// Row `y` without its stride padding.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        let start = y as usize * self.stride;
        let row_bytes = self.width as usize * self.format.bytes_per_pixel();
        (y < self.height)
            .then(|| self.pixels().get(start..start + row_bytes))
            .flatten()
    }

    /// The red, green and blue at (`x`, `y`); black outside the image.
    pub fn rgb_at(&self, x: u32, y: u32) -> [u8; 3] {
        let bytes_per_pixel = self.format.bytes_per_pixel();
        let start = x as usize * bytes_per_pixel;
        self.row(y)
            .and_then(|row| row.get(start..start + bytes_per_pixel))
            .map_or([0; 3], |pixel| self.format.rgb(pixel))
    }

    /// A packed RGB copy, for a runtime that takes only that.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let bytes_per_pixel = self.format.bytes_per_pixel();
        let mut rgb = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for y in 0..self.height {
            if let Some(row) = self.row(y) {
                for pixel in row.chunks_exact(bytes_per_pixel) {
                    rgb.extend_from_slice(&self.format.rgb(pixel));
                }
            }
        }
        rgb
    }
}

/// Input that is not text. Local backends take raw pixels; only remote ones encode.
#[derive(Clone, Debug)]
pub enum Media {
    Image(Image),
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
        let image = Image::new(2, 2, 8, PixelFormat::Rgb8, pixels.into()).unwrap();
        assert_eq!(image.row(1).unwrap(), [8, 9, 10, 11, 12, 13]);
        assert_eq!(image.row(2), None);
    }

    #[test]
    fn the_last_row_needs_no_padding() {
        assert!(Image::new(2, 2, 8, PixelFormat::Rgb8, vec![0; 14].into()).is_ok());
        assert!(Image::new(2, 2, 8, PixelFormat::Rgb8, vec![0; 13].into()).is_err());
    }

    #[test]
    fn a_stride_shorter_than_a_row_is_refused() {
        assert!(Image::new(2, 1, 7, PixelFormat::Xrgb8888, vec![0; 64].into()).is_err());
    }

    #[test]
    fn compositor_pixels_read_as_rgb() {
        let xrgb = Image::new(1, 1, 4, PixelFormat::Xrgb8888, vec![3, 2, 1, 0].into()).unwrap();
        assert_eq!(xrgb.rgb_at(0, 0), [1, 2, 3]);
        assert_eq!(xrgb.to_rgb8(), [1, 2, 3]);
        let xbgr = Image::new(1, 1, 4, PixelFormat::Xbgr8888, vec![1, 2, 3, 0].into()).unwrap();
        assert_eq!(xbgr.rgb_at(0, 0), [1, 2, 3]);
        assert_eq!(xbgr.rgb_at(1, 0), [0, 0, 0]);
    }
}
