//! Everything the engine keeps: one SQLCipher database for events, segments, episodes and the
//! text seen on screen, with FTS5 over the text, written by one thread in coalesced
//! transactions and read by anyone with the key. The key lives beside the database, readable by
//! the owner only, and is the only thing that opens it.

mod key;
mod migrations;
mod queries;
mod store;
mod writer;

pub use key::{KEY_FILE, KeyError, load_or_create_key};
pub use queries::{EventRow, ForgetOutcome, SegmentRow, TextHit, TextLine};
pub use store::{DATABASE_FILE, Store, StoreError};
pub use writer::{Write, WriteHandle, Writer};
