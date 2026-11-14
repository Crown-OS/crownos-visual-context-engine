//! What the engine makes of observations, as pure state machines: the timeline merges
//! heartbeats, the segmenter turns attention into [`Segment`]s, the classifier names them, the
//! episode builder chains them, and working memory answers `current_context`. No I/O, no clocks:
//! every function takes the time it is told, so a day replays in milliseconds.
//!
//! [`Segment`]: context_core::Segment

mod classifier;
mod correlator;
mod doc_key;
mod engine;
mod episodes;
mod heartbeat;
mod importance;
mod segmenter;
mod working_memory;

pub use classifier::classify;
pub use correlator::{Correlator, Trigger};
pub use doc_key::{canonical_url, derive_doc_key, git_root, normalize_title, terminal_directory};
pub use engine::{Engine, EngineEvent};
pub use episodes::{EpisodeBuilder, EpisodeEvent};
pub use heartbeat::{Heartbeat, merge_heartbeat};
pub use importance::importance;
pub use segmenter::{SegmentEvent, Segmenter, SegmenterConfig};
pub use working_memory::WorkingMemory;
