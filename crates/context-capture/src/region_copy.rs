//! What a model reads: the rows of one region, written once into a new memfd that is then
//! sealed against every change. The capture memfd is zeroed right after the copies are made,
//! and a copy goes away when its last descriptor closes.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use context_shm::REQUIRED_SEALS;
use rustix::fs::{MemfdFlags, SealFlags};

use crate::frame::CapturedFrame;
use crate::geometry::Rect;
use crate::util::row_buffer::RowBuffer;

const XRGB_BYTES_PER_PIXEL: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum RegionCopyError {
    #[error("the region {0:?} is outside the frame")]
    OutsideFrame(Rect),
    #[error("cannot write the region copy: {0}")]
    Write(#[from] std::io::Error),
    #[error("cannot seal the region copy: {0}")]
    Seal(#[from] rustix::io::Errno),
}

/// `XRGB8888` rows of `bounds`, packed with no padding.
#[derive(Debug)]
pub struct RegionCopy {
    memfd: OwnedFd,
    bounds: Rect,
}

impl RegionCopy {
    /// # Errors
    ///
    /// Fails when the region is outside the frame, or the memfd cannot be written or sealed.
    pub fn of(frame: &CapturedFrame, bounds: Rect) -> Result<Self, RegionCopyError> {
        let memfd = rustix::fs::memfd_create(
            "crown-context-region",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let mut file = File::from(memfd);
        let mut written = 0;
        let sealed = copy_rows(&mut file, frame, bounds, &mut written).and_then(|()| {
            rustix::fs::fcntl_add_seals(&file, REQUIRED_SEALS | SealFlags::SEAL)
                .map_err(RegionCopyError::from)
        });
        if let Err(error) = sealed {
            zero_written(&file, written);
            return Err(error);
        }
        Ok(Self {
            memfd: OwnedFd::from(file),
            bounds,
        })
    }

    /// Where the region is, in frame pixels.
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    pub const fn stride(&self) -> u32 {
        self.bounds.width.saturating_mul(XRGB_BYTES_PER_PIXEL)
    }

    pub fn memfd(&self) -> BorrowedFd<'_> {
        self.memfd.as_fd()
    }
}

fn copy_rows(
    file: &mut File,
    frame: &CapturedFrame,
    bounds: Rect,
    written: &mut usize,
) -> Result<(), RegionCopyError> {
    let mut rows = RowBuffer::default();
    for y in bounds.y..bounds.bottom() {
        let row = rows
            .read(frame, bounds.x, y, bounds.width)
            .ok_or(RegionCopyError::OutsideFrame(bounds))?;
        file.write_all(row)?;
        *written += row.len();
    }
    Ok(())
}

/// A copy that failed before its seal still holds rows: they are overwritten before the memfd
/// closes, so its pages do not go back to the kernel with pixels on them.
fn zero_written(file: &File, written: usize) {
    const CHUNK: usize = 64 * 1024;
    let zeros = [0_u8; CHUNK];
    let mut offset = 0;
    while offset < written {
        let length = (written - offset).min(CHUNK);
        match rustix::io::pwrite(file, zeros.get(..length).unwrap_or(&zeros), offset as u64) {
            Ok(0) | Err(_) => return,
            Ok(count) => offset += count,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_frames::{INK_BGRX, frame_with, paint_rect};

    #[test]
    fn a_copy_holds_the_region_rows_and_is_sealed_against_writes() {
        let frame = frame_with(
            16,
            8,
            |pixels| paint_rect(pixels, 16, Rect::new(2, 1, 2, 1)),
            vec![],
        );
        let copy = RegionCopy::of(&frame, Rect::new(2, 1, 3, 2)).unwrap();
        let mut contents = vec![0; 3 * 2 * 4];
        assert_eq!(
            rustix::io::pread(copy.memfd(), &mut contents, 0).unwrap(),
            24
        );
        assert_eq!(contents[..4], INK_BGRX);
        assert!(rustix::io::write(copy.memfd(), &[0]).is_err());
        assert!(RegionCopy::of(&frame, Rect::new(10, 0, 8, 1)).is_err());
    }
}
