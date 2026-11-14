//! The wire contract of the `crownos-visual-context` service: what the shell, agents, the CLI and
//! the Inspector ask the engine, and what it tells them. Postcard is positional, so fields are
//! only ever appended; a breaking change renames the service.

mod types;

pub use types::*;

pub const SERVICE: &str = "crownos-visual-context";

crownos_ipc::protocol! {
    /// The Context API, v1.
    pub mod context: "crownos-visual-context" {
        /// Whether the engine observes, and how it is doing.
        fn status() -> Status;
        /// What the user is doing right now.
        fn current_context() -> CurrentContext;
        /// Search over segments and episodes.
        fn search(query: SearchQuery) -> SearchResults;
        /// Segments and episodes over a time range.
        fn timeline(query: TimelineQuery) -> Timeline;
        /// One episode with its segments.
        fn episode(id: EpisodeId) -> Option<EpisodeView>;
        /// Why the engine remembers something.
        fn explain(memory: MemoryRef) -> Explanation;
        /// A memory asserted by the user or an agent.
        fn remember(assertion: MemoryAssertion) -> MemoryRef;
        /// Delete evidence and whatever rested only on it.
        fn forget(request: ForgetRequest) -> ForgetReport;
        /// Stop observing for a while; 0 means until resumed.
        notify pause(seconds: u64);
        /// Observe again.
        notify resume();

        /// The current context changed; coalesced, at most a few per second.
        event ContextChanged { context: CurrentContext };
        /// A segment closed and is now part of the timeline.
        event SegmentClosed { segment: SegmentView };
        /// Observation started or stopped.
        event CaptureStateChanged { state: CaptureState };
    }
}
