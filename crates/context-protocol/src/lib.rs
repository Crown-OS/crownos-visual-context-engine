//! The wire contract of the `crownos-visual-context` service: what the shell, agents, the CLI and
//! the Inspector ask the engine, and what it tells them. Postcard is positional, so fields are
//! only ever appended; a breaking change renames the service.

mod inference_types;
mod types;

pub use inference_types::*;
pub use types::*;

pub const SERVICE: &str = "crownos-visual-context";
pub const INFERENCE_SERVICE: &str = "crownos-visual-context-infer";

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

    /// The inference worker, v1: tasks by name, models chosen inside. Each image comes as
    /// one sealed memfd (`pixels`), mapped read-only.
    pub mod inference: "crownos-visual-context-infer" {
        /// Devices, routes, loaded models, the embedding space and the prompt templates.
        fn capabilities() -> InferCapabilities;
        /// The text lines of an image, in reading order.
        fn ocr(job: JobTicket, image: SharedImage) fds(pixels) -> InferReply<Vec<RecognizedLine>>;
        /// At most 768 KiB of vectors per call; split larger batches.
        fn embed_text(job: JobTicket, texts: Vec<String>, purpose: TextPurpose) -> InferReply<EmbeddedBatch>;
        fn rerank(job: JobTicket, query: String, documents: Vec<String>) -> InferReply<Vec<f32>>;
        /// A named prompt template filled with `vars`; `schema` (JSON Schema text) forces JSON.
        fn generate(job: JobTicket, template: String, vars: Vec<(String, String)>, schema: Option<String>, max_tokens: u32) -> InferReply<String>;
        fn generate_with_image(job: JobTicket, template: String, vars: Vec<(String, String)>, schema: Option<String>, max_tokens: u32, image: SharedImage) fds(pixels) -> InferReply<String>;
        fn describe_screen(job: JobTicket, image: SharedImage, app_name: String, window_title: String) fds(pixels) -> InferReply<ScreenSummary>;
        /// Unloads every model now; gives how many.
        fn unload() -> u32;
        /// Ends the job `job_id` of this connection's client: a job that waits does not run,
        /// and its memfds are unmapped and closed when it ends. Gives whether the job was known.
        fn cancel(job_id: u64) -> bool;
    }
}
