use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;

use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};

use crate::{REQUIRED_SEALS, ShmError};

/// A sealed memfd with a copy of `bytes`, ready to pass over a socket. It is filled with
/// write(2), never mapped writable, so `F_SEAL_WRITE` can go on; the seals stop all later
/// changes, and a receiver can map it read-only and trust what it reads.
pub fn share_read_only(name: &CStr, bytes: &[u8]) -> Result<OwnedFd, ShmError> {
    let memfd = memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;
    let mut file = File::from(memfd);
    file.write_all(bytes)?;
    let memfd = OwnedFd::from(file);
    fcntl_add_seals(&memfd, REQUIRED_SEALS | SealFlags::SEAL)?;
    Ok(memfd)
}
