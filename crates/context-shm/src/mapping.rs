use std::fmt;
use std::os::fd::OwnedFd;
use std::ptr::{self, NonNull};

use rustix::fs::{fcntl_get_seals, fstat};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};

use crate::{REQUIRED_SEALS, ShmError};

/// A sealed memfd mapped read-only. The seals guarantee that the bytes never change while
/// this mapping lives.
pub struct ReadOnlyMapping {
    address: NonNull<u8>,
    length: usize,
}

// SAFETY: the mapping is read-only and its file is sealed against writes and resizes, so the
// bytes are immutable for the life of the mapping and any thread may read them.
#[allow(unsafe_code)]
unsafe impl Send for ReadOnlyMapping {}
// SAFETY: as for `Send`: shared reads of immutable memory.
#[allow(unsafe_code)]
unsafe impl Sync for ReadOnlyMapping {}

impl ReadOnlyMapping {
    /// Maps `memfd` after a check that it carries [`REQUIRED_SEALS`]. The file descriptor
    /// closes when this returns; the mapping stays valid.
    pub fn map(memfd: &OwnedFd) -> Result<Self, ShmError> {
        if !fcntl_get_seals(memfd)?.contains(REQUIRED_SEALS) {
            return Err(ShmError::NotSealed);
        }
        let length = usize::try_from(fstat(memfd)?.st_size).map_err(|_| ShmError::Empty)?;
        if length == 0 {
            return Err(ShmError::Empty);
        }
        // SAFETY: a fresh shared read-only mapping of a file that is sealed against shrinking,
        // so every page in `length` stays backed and readable until `munmap` in `Drop`.
        #[allow(unsafe_code)]
        let address = unsafe {
            mmap(
                ptr::null_mut(),
                length,
                ProtFlags::READ,
                MapFlags::SHARED,
                memfd,
                0,
            )?
        };
        let address = NonNull::new(address.cast::<u8>()).ok_or(ShmError::Empty)?;
        Ok(Self { address, length })
    }
}

impl AsRef<[u8]> for ReadOnlyMapping {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: `address` points to `length` readable bytes that live until `Drop`, and the
        // seals stop every write, so no mutable alias exists.
        #[allow(unsafe_code)]
        unsafe {
            std::slice::from_raw_parts(self.address.as_ptr(), self.length)
        }
    }
}

impl Drop for ReadOnlyMapping {
    fn drop(&mut self) {
        // SAFETY: this is the mapping `map` made, and no slice of it outlives `self`.
        #[allow(unsafe_code)]
        let _ = unsafe { munmap(self.address.as_ptr().cast(), self.length) };
    }
}

impl fmt::Debug for ReadOnlyMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ReadOnlyMapping({} bytes)", self.length)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs::File;
    use std::io::Write;

    use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};

    use super::*;
    use crate::share_read_only;

    #[test]
    fn a_shared_image_maps_with_the_same_bytes() {
        let memfd = share_read_only(c"frame", &[1, 2, 3, 4]).unwrap();
        let mapping = ReadOnlyMapping::map(&memfd).unwrap();
        drop(memfd);
        assert_eq!(mapping.as_ref(), [1, 2, 3, 4]);
    }

    #[test]
    fn a_shared_memfd_cannot_be_written_again() {
        let memfd = share_read_only(c"frame", &[1, 2, 3, 4]).unwrap();
        let mut file = File::from(memfd);
        assert!(file.write_all(&[9]).is_err());
    }

    #[test]
    fn an_unsealed_or_partly_sealed_memfd_is_refused() {
        let memfd = memfd_create(c"open", MemfdFlags::ALLOW_SEALING).unwrap();
        let mut file = File::from(memfd);
        file.write_all(&[1, 2, 3]).unwrap();
        let memfd = OwnedFd::from(file);
        assert!(matches!(
            ReadOnlyMapping::map(&memfd),
            Err(ShmError::NotSealed)
        ));
        fcntl_add_seals(&memfd, SealFlags::SHRINK | SealFlags::GROW).unwrap();
        assert!(matches!(
            ReadOnlyMapping::map(&memfd),
            Err(ShmError::NotSealed)
        ));
    }
}
