use std::os::fd::OwnedFd;

use context_infer::{Image, InferError, PixelFormat, SharedBytes};
use context_protocol::{SharedImage, WirePixelFormat};
use context_shm::{ReadOnlyMapping, share_read_only};

/// Pixels in a memfd sealed against every change, with their layout.
#[derive(Debug)]
pub struct SealedFrame {
    memfd: OwnedFd,
    layout: SharedImage,
}

impl SealedFrame {
    /// A frame that the capture path already wrote and sealed: no copy. The seals and the
    /// size are checked here, so a bad frame fails before it is sent.
    pub fn from_sealed_memfd(
        memfd: OwnedFd,
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
    ) -> Result<Self, InferError> {
        let limit = stride as usize * height as usize;
        let mapping = ReadOnlyMapping::map(&memfd, limit)
            .map_err(|error| InferError::InvalidInput(error.to_string()))?;
        Image::new(
            width,
            height,
            stride as usize,
            format,
            SharedBytes::new(mapping),
        )
        .map_err(|error| InferError::InvalidInput(error.to_string()))?;
        Ok(Self {
            memfd,
            layout: layout(width, height, stride, format),
        })
    }

    /// A sealed copy of `image`, written once into a new memfd.
    pub fn copy_of(image: &Image) -> Result<Self, InferError> {
        let stride = u32::try_from(image.stride())
            .map_err(|_| InferError::InvalidInput("a stride above 4 GiB".into()))?;
        let memfd = share_read_only(c"crown-context-frame", image.pixels())
            .map_err(|error| InferError::Backend(format!("sharing a frame: {error}")))?;
        Ok(Self {
            memfd,
            layout: layout(image.width(), image.height(), stride, image.format()),
        })
    }

    pub(crate) const fn layout(&self) -> SharedImage {
        self.layout
    }

    /// A second descriptor of the same memfd, for one request.
    pub(crate) fn descriptor(&self) -> Result<OwnedFd, InferError> {
        self.memfd
            .try_clone()
            .map_err(|error| InferError::Backend(format!("duplicating the frame: {error}")))
    }
}

const fn layout(width: u32, height: u32, stride: u32, format: PixelFormat) -> SharedImage {
    SharedImage {
        width,
        height,
        stride,
        format: match format {
            PixelFormat::Rgb8 => WirePixelFormat::Rgb8,
            PixelFormat::Xrgb8888 => WirePixelFormat::Xrgb8888,
            PixelFormat::Xbgr8888 => WirePixelFormat::Xbgr8888,
        },
    }
}
