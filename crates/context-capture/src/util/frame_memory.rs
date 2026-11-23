//! The only memory that holds pixels. One sealed memfd for each capture, faulted in at once so
//! its pages are charged to this process's cgroup (where swap is off), locked in RAM when the
//! limit allows, kept out of core dumps and forks, and overwritten with zeros before it is
//! unmapped and closed.
//!
//! The compositor can write to the same pages, so no Rust reference to the mapping is ever
//! made: bytes are copied in and out through the raw pointer.

use std::ffi::c_void;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::fs::{MemfdFlags, SealFlags};
use rustix::io::Errno;
use rustix::mm::{Advice, MapFlags, ProtFlags};

const MEMFD_NAME: &str = "crown-context-frame";

const WORD: usize = std::mem::size_of::<u64>();

static MLOCK_FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub enum FrameMemoryError {
    #[error("a frame of zero bytes cannot be mapped")]
    Empty,
    #[error("cannot create the frame memfd: {0}")]
    Create(Errno),
    #[error("cannot size the frame memfd to {length} bytes: {errno}")]
    Resize { length: usize, errno: Errno },
    #[error("cannot map the frame memfd: {0}")]
    Map(Errno),
    #[error("cannot keep the frame out of core dumps and forks: {0}")]
    Advise(Errno),
    #[error("cannot seal the frame memfd: {0}")]
    Seal(Errno),
}

/// A shared, writable mapping of one memfd. Dropping it zeroes every byte, then unmaps and
/// closes, so each exit path deletes the pixels.
#[derive(Debug)]
pub struct FrameMemory {
    memfd: OwnedFd,
    mapping: NonNull<u8>,
    length: usize,
    locked: bool,
    sealed: bool,
}

// SAFETY: the mapping lives as long as this value. No reference into it is handed out; every
// access is a bounds-checked copy or a volatile write through the raw pointer, which another
// thread or the compositor writing the same pages cannot make unsound.
unsafe impl Send for FrameMemory {}
// SAFETY: as above.
unsafe impl Sync for FrameMemory {}

impl FrameMemory {
    /// # Errors
    ///
    /// Fails when the memfd cannot be created, sized, mapped or advised. A failed `mlock` is
    /// not an error: the frame is still analysed and zeroed, and the failure is logged once.
    pub fn allocate(length: usize) -> Result<Self, FrameMemoryError> {
        if length == 0 {
            return Err(FrameMemoryError::Empty);
        }
        let memfd = create_sealable_memfd()?;
        rustix::fs::ftruncate(&memfd, length as u64)
            .map_err(|errno| FrameMemoryError::Resize { length, errno })?;
        // SAFETY: a fresh shared mapping of a memfd we own, `length` bytes long, at an address
        // the kernel picks. Nothing else refers to that range.
        let address = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                length,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &memfd,
                0,
            )
        }
        .map_err(FrameMemoryError::Map)?;
        let Some(mapping) = NonNull::new(address.cast::<u8>()) else {
            return Err(FrameMemoryError::Map(Errno::NOMEM));
        };
        let mut memory = Self {
            memfd,
            mapping,
            length,
            locked: false,
            sealed: false,
        };
        memory.advise(Advice::LinuxDontDump)?;
        memory.advise(Advice::LinuxDontFork)?;
        if memory.advise(Advice::LinuxPopulateWrite).is_err() {
            memory.wipe();
        }
        // SAFETY: the range is our live mapping.
        match unsafe { rustix::mm::mlock(memory.address(), length) } {
            Ok(()) => memory.locked = true,
            Err(errno) => {
                if !MLOCK_FAILURE_LOGGED.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        %errno,
                        length,
                        "cannot lock a frame in RAM (RLIMIT_MEMLOCK); frames are still zeroed after analysis"
                    );
                }
            }
        }
        Ok(memory)
    }

    /// Stops all new writable mappings and any change of size, from every holder of the memfd.
    /// This mapping stays writable, so the frame can still be zeroed.
    ///
    /// # Errors
    ///
    /// Fails when the kernel refuses the seals.
    pub fn seal(&mut self) -> Result<(), FrameMemoryError> {
        rustix::fs::fcntl_add_seals(
            &self.memfd,
            SealFlags::FUTURE_WRITE | SealFlags::SHRINK | SealFlags::GROW,
        )
        .map_err(FrameMemoryError::Seal)?;
        self.sealed = true;
        Ok(())
    }

    /// The memfd, for a `wl_shm` pool before the capture.
    pub fn memfd(&self) -> BorrowedFd<'_> {
        self.memfd.as_fd()
    }

    /// The memfd for a reader in another process, once it is sealed.
    pub fn sealed_memfd(&self) -> Option<BorrowedFd<'_>> {
        self.sealed.then(|| self.memfd.as_fd())
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub const fn is_locked(&self) -> bool {
        self.locked
    }

    pub const fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Copies `bytes` in at `offset`. Returns `false`, and copies nothing, when they do not fit.
    pub fn write_at(&mut self, offset: usize, bytes: &[u8]) -> bool {
        if offset
            .checked_add(bytes.len())
            .is_none_or(|end| end > self.length)
        {
            return false;
        }
        // SAFETY: the range is inside the live mapping, and `bytes` is not in it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.mapping.as_ptr().add(offset),
                bytes.len(),
            );
        }
        true
    }

    /// Copies `out.len()` bytes out from `offset`. Returns `false`, and copies nothing, when the
    /// range is outside the frame.
    pub fn read_at(&self, offset: usize, out: &mut [u8]) -> bool {
        if offset
            .checked_add(out.len())
            .is_none_or(|end| end > self.length)
        {
            return false;
        }
        // SAFETY: the range is inside the live mapping, and `out` is not in it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.mapping.as_ptr().add(offset),
                out.as_mut_ptr(),
                out.len(),
            );
        }
        true
    }

    /// Overwrites every byte with zero. The writes are volatile, so the compiler keeps them
    /// even when nothing reads the memory again.
    pub fn wipe(&self) {
        let words = self.length / WORD;
        let base = self.mapping.as_ptr();
        for index in 0..words {
            // SAFETY: `mmap` returns page-aligned memory, so every word is aligned and inside.
            unsafe { base.cast::<u64>().add(index).write_volatile(0) };
        }
        for offset in words * WORD..self.length {
            // SAFETY: inside the live mapping.
            unsafe { base.add(offset).write_volatile(0) };
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }

    const fn address(&self) -> *mut c_void {
        self.mapping.as_ptr().cast::<c_void>()
    }

    fn advise(&self, advice: Advice) -> Result<(), FrameMemoryError> {
        // SAFETY: the range is our live mapping; these advices change no contents.
        unsafe { rustix::mm::madvise(self.address(), self.length, advice) }
            .map_err(FrameMemoryError::Advise)
    }
}

impl Drop for FrameMemory {
    fn drop(&mut self) {
        self.wipe();
        // SAFETY: the mapping is ours, and no reference to it outlives `&mut self`.
        if let Err(errno) = unsafe { rustix::mm::munmap(self.address(), self.length) } {
            tracing::error!(%errno, "cannot unmap a zeroed frame");
        }
    }
}

/// `MFD_NOEXEC_SEAL` needs Linux 6.3; an older kernel gets a memfd that is never executable
/// because nothing maps it with `PROT_EXEC`.
fn create_sealable_memfd() -> Result<OwnedFd, FrameMemoryError> {
    let flags = MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING;
    rustix::fs::memfd_create(MEMFD_NAME, flags | MemfdFlags::NOEXEC_SEAL)
        .or_else(|errno| match errno {
            Errno::INVAL => rustix::fs::memfd_create(MEMFD_NAME, flags),
            other => Err(other),
        })
        .map_err(FrameMemoryError::Create)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_frame_refuses_new_writers_and_growth() {
        let mut memory = FrameMemory::allocate(4096).unwrap();
        assert!(memory.write_at(0, &[0xAB; 4096]));
        memory.seal().unwrap();
        assert!(memory.sealed_memfd().is_some());
        assert!(rustix::fs::ftruncate(memory.memfd(), 8192).is_err());
        assert!(rustix::fs::ftruncate(memory.memfd(), 1024).is_err());
        assert!(rustix::io::write(memory.memfd(), &[1]).is_err());
        assert!(memory.write_at(0, &[0]));
        let mut first = [0xFF; 2];
        assert!(memory.read_at(0, &mut first));
        assert_eq!(first, [0, 0xAB]);
        assert!(!memory.read_at(4095, &mut first));
    }

    #[test]
    fn a_frame_larger_than_the_memlock_limit_still_maps() {
        let memory = FrameMemory::allocate(64 << 20).unwrap();
        assert_eq!(memory.len(), 64 << 20);
    }

    #[test]
    fn dropping_a_frame_zeroes_the_memfd() {
        let mut memory = FrameMemory::allocate(4096).unwrap();
        assert!(memory.write_at(0, &[0x5A; 4096]));
        let reader = rustix::io::dup(memory.memfd()).unwrap();
        drop(memory);
        let mut contents = vec![0xFF; 4096];
        let read = rustix::io::pread(&reader, &mut contents, 0).unwrap();
        assert_eq!(read, 4096);
        assert!(contents.iter().all(|byte| *byte == 0));
    }
}
