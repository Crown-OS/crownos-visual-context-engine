//! Images go from a capture process to crown-context-inferd in a memfd. The sender writes the
//! bytes and seals the memfd so that nobody can change or resize it; the receiver checks the
//! seals and maps the file read-only. No pixel is copied on the way, and no pixel touches a
//! disk.

mod error;
mod mapping;
mod sender;

pub use error::ShmError;
pub use mapping::ReadOnlyMapping;
pub use sender::share_read_only;

use rustix::fs::SealFlags;

/// The seals [`share_read_only`] puts on: no new writes, no size change.
pub const REQUIRED_SEALS: SealFlags = SealFlags::FUTURE_WRITE
    .union(SealFlags::SHRINK)
    .union(SealFlags::GROW);

/// Either seal stops writes: `WRITE` also refuses existing writable mappings.
const WRITE_SEALS: SealFlags = SealFlags::WRITE.union(SealFlags::FUTURE_WRITE);
const SIZE_SEALS: SealFlags = SealFlags::SHRINK.union(SealFlags::GROW);
