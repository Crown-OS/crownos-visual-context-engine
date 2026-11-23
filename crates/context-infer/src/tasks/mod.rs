//! What callers ask for. A caller names a task, never a model or a runtime.

mod describe;
mod routed;
mod summarize;
mod templates;

use crate::capability::{EmbedPurpose, EmbedSpace, TextLine};
use crate::error::InferError;
use crate::media::Image;
use crate::request::Job;
use crate::router::EmbeddedTexts;

pub use describe::{ContentKind, ScreenDescription};
pub use routed::RoutedTasks;
pub use summarize::{Entity, Fact, SegmentDigest, SegmentSummary, summarize_prompt};
pub use templates::{PROMPT_TEMPLATES, PromptTemplate};

pub trait ContextTasks: Send + Sync {
    /// One summary per segment, in input order.
    fn summarize_segments(
        &self,
        segments: &[SegmentDigest],
        job: Job,
    ) -> Result<Vec<SegmentSummary>, InferError>;

    fn describe_screen(
        &self,
        screen: &Image,
        app_name: &str,
        window_title: &str,
        job: Job,
    ) -> Result<ScreenDescription, InferError>;

    fn recognize_text(&self, image: &Image, job: Job) -> Result<Vec<TextLine>, InferError>;

    fn embed_text(
        &self,
        texts: Vec<String>,
        purpose: EmbedPurpose,
        job: Job,
    ) -> Result<EmbeddedTexts, InferError>;

    /// The space [`ContextTasks::embed_text`] uses now; `None` when no embedder is usable.
    fn embed_space(&self) -> Option<EmbedSpace>;
}

#[cfg(test)]
mod tests;
