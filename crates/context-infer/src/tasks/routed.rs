use std::sync::Arc;

use serde::de::DeserializeOwned;

use super::ContextTasks;
use super::describe::{DESCRIBE_SCREEN_SYSTEM_PROMPT, ScreenDescription, describe_screen_prompt};
use super::summarize::{
    SUMMARIZE_SYSTEM_PROMPT, SegmentDigest, SegmentSummaries, SegmentSummary, summarize_prompt,
};
use crate::capability::{EmbedPurpose, EmbedSpace, TextLine};
use crate::error::InferError;
use crate::media::{Image, Media};
use crate::request::{GenRequest, Job, Message, Output, Sampling};
use crate::router::{EmbeddedTexts, Router};
use crate::task::Task;

const SUMMARY_TOKENS_PER_SEGMENT: u32 = 400;
const DESCRIPTION_TOKENS: u32 = 256;

/// [`ContextTasks`] in this process, through a [`Router`].
#[derive(Clone, Debug)]
pub struct RoutedTasks {
    router: Arc<Router>,
}

impl RoutedTasks {
    pub const fn new(router: Arc<Router>) -> Self {
        Self { router }
    }

    fn generate_json<T: DeserializeOwned>(
        &self,
        task: Task,
        request: GenRequest,
    ) -> Result<T, InferError> {
        let generation = self.router.generate(task, request)?;
        serde_json::from_str(&generation.text)
            .map_err(|error| InferError::Schema(error.to_string()))
    }
}

impl ContextTasks for RoutedTasks {
    fn summarize_segments(
        &self,
        segments: &[SegmentDigest],
        job: Job,
    ) -> Result<Vec<SegmentSummary>, InferError> {
        if segments.is_empty() {
            return Ok(Vec::new());
        }
        let segment_count = u32::try_from(segments.len()).unwrap_or(u32::MAX);
        let request = GenRequest {
            messages: vec![
                Message::system(SUMMARIZE_SYSTEM_PROMPT),
                Message::user(summarize_prompt(segments)),
            ],
            media: Vec::new(),
            output: Output::json_for::<SegmentSummaries>(),
            sampling: Sampling::default(),
            max_tokens: SUMMARY_TOKENS_PER_SEGMENT.saturating_mul(segment_count),
            job,
        };
        let summaries: SegmentSummaries = self.generate_json(Task::SummarizeSegments, request)?;
        if summaries.segments.len() != segments.len() {
            return Err(InferError::Schema(format!(
                "{} summaries for {} segments",
                summaries.segments.len(),
                segments.len()
            )));
        }
        Ok(summaries.segments)
    }

    fn describe_screen(
        &self,
        screen: &Image,
        app_name: &str,
        window_title: &str,
        job: Job,
    ) -> Result<ScreenDescription, InferError> {
        let request = GenRequest {
            messages: vec![
                Message::system(DESCRIBE_SCREEN_SYSTEM_PROMPT),
                Message::user(describe_screen_prompt(app_name, window_title)),
            ],
            media: vec![Media::Image(screen.clone())],
            output: Output::json_for::<ScreenDescription>(),
            sampling: Sampling::default(),
            max_tokens: DESCRIPTION_TOKENS,
            job,
        };
        self.generate_json(Task::DescribeScreen, request)
    }

    fn recognize_text(&self, image: &Image, job: Job) -> Result<Vec<TextLine>, InferError> {
        self.router.recognize_text(image.clone(), &job)
    }

    fn embed_text(
        &self,
        texts: Vec<String>,
        purpose: EmbedPurpose,
        job: Job,
    ) -> Result<EmbeddedTexts, InferError> {
        self.router.embed(texts, purpose, &job)
    }

    fn embed_space(&self) -> Option<EmbedSpace> {
        self.router.embed_space()
    }
}
