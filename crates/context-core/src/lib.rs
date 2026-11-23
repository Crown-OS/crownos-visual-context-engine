//! The vocabulary of the visual context engine: what the compositor and the collectors observe
//! ([`Observation`]), the windows those observations are about ([`WindowRecord`]), and what the
//! engine makes of them ([`Segment`], [`Episode`]). Plain data, postcard-friendly, no I/O.

mod ids;
mod observation;
mod segment;
mod time;
mod window;

pub use ids::{
    EntityId, EpisodeId, EventId, FactId, FrameId, MemoryKind, MemoryRef, ProjectId, SegmentId,
};
pub use observation::{
    FrameRecord, InputCounts, Observation, ObservationKind, PauseOrigin, SelectionKind,
    SessionState, TextSource, Trigger,
};
pub use segment::{
    ActivityLabel, DocKey, Episode, EpisodeLevel, InputProfile, Segment, SegmentState,
};
pub use time::{Micros, TimeRange, Timestamp};
pub use window::{
    AppRef, ContentType, Placement, Sandbox, Sensitivity, WindowIdent, WindowRecord, WindowState,
};
