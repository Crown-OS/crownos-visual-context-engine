use serde::{Deserialize, Serialize};

macro_rules! id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

id!(
    /// One row of the event log.
    EventId
);
id!(
    /// One activity segment: time on one document in one app.
    SegmentId
);
id!(
    /// One episode: a chain of segments that belong to one task, day or week.
    EpisodeId
);
id!(EntityId);
id!(FactId);
id!(FrameId);
id!(
    /// A project is an entity; this is its id in that table.
    ProjectId
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryKind {
    Segment,
    Episode,
    Entity,
    Fact,
    Preference,
    Procedure,
    Note,
}

/// Anything the engine remembers, addressable for `explain`, `correct` and `forget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryRef {
    pub kind: MemoryKind,
    pub id: u64,
}
