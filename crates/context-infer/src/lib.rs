//! The model layer of the visual context engine. Callers ask [`ContextTasks`] for a task; the
//! [`Router`] picks a model, runtime and device from the [`Registry`] and falls through to
//! the next candidate on a failure. Backend crates implement [`RuntimeBackend`] and the
//! capability traits. This crate has no inference runtime and no unsafe code.

pub mod capability;
mod error;
mod media;
pub mod registry;
mod request;
mod router;
mod task;
mod tasks;
mod util;

pub use capability::{
    Embed, EmbedPurpose, EmbedSpace, Embeddings, Generate, GenerateCapabilities, Ocr, Rerank,
    TextBounds, TextLine, Transcribe,
};
pub use crownos_config::system::ai::{Device, Runtime};
pub use error::InferError;
pub use media::{Image, ImageLayoutError, Media, PixelFormat, SharedBytes};
pub use registry::{Capability, Registry};
pub use request::{
    GenRequest, Generation, Job, JobCancel, JobLease, Message, Output, Priority, Role, Sampling,
};
pub use router::{
    BenchmarkCache, BreakerSettings, EmbeddedTexts, LoadSpec, Policy, Router, RouterSettings,
    RuntimeBackend, benchmark_fingerprint, default_benchmark_path,
};
pub use task::Task;
pub use tasks::{
    ContentKind, ContextTasks, Entity, Fact, PROMPT_TEMPLATES, PromptTemplate, RoutedTasks,
    ScreenDescription, SegmentDigest, SegmentSummaries, SegmentSummary, summarize_prompt,
};
