//! Images go from a capture process to crown-context-inferd in a memfd. The sender writes the
//! bytes and seals the memfd so that nobody can change or resize it; the receiver checks the
//! seals and maps the file read-only. No pixel is copied on the way, and no pixel touches a
//! disk. Needs Linux 3.17 or later (memfd sealing).

mod error;
mod mapping;
mod sender;

pub use error::ShmError;
pub use mapping::ReadOnlyMapping;
pub use sender::share_read_only;

use rustix::fs::SealFlags;

/// The seals a shared memfd must carry: no write of any kind and no size change. `WRITE` can
/// only go on when no writable mapping exists, so none can change the bytes later.
pub const REQUIRED_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::SHRINK)
    .union(SealFlags::GROW);
