use std::path::PathBuf;
use std::time::Duration;

use context_infer::{
    ContextTasks, EmbedPurpose, EmbedSpace, EmbeddedTexts, Image, InferError, Job, Output,
    ScreenDescription, SegmentDigest, SegmentSummaries, SegmentSummary, TextLine, summarize_prompt,
};
use context_protocol::InferReply;
use context_protocol::inference::Client;
use crownos_ipc::Pending;

use crate::convert;
use crate::frame::SealedFrame;

/// Time past the job budget that the client waits for inferd's own `Timeout`.
const REPLY_SLACK: Duration = Duration::from_secs(5);
const SUMMARY_TOKENS_PER_SEGMENT: u32 = 400;

/// Talks to crown-context-inferd. Each call opens its own connection, so calls from many
/// threads never wait for each other here; inferd orders them.
#[derive(Debug, Clone, Default)]
pub struct InferdClient {
    socket_dir: Option<PathBuf>,
}

impl InferdClient {
    /// `socket_dir` defaults to `$XDG_RUNTIME_DIR/crownos`.
    pub const fn new(socket_dir: Option<PathBuf>) -> Self {
        Self { socket_dir }
    }

    /// OCR of a frame that is already in a sealed memfd.
    pub fn recognize_frame(
        &self,
        frame: &SealedFrame,
        job: Job,
    ) -> Result<Vec<TextLine>, InferError> {
        let pixels = frame.descriptor()?;
        let lines = self.call(job, |client, ticket| {
            client.ocr(ticket, frame.layout(), pixels)
        })?;
        Ok(convert::lines(lines))
    }

    /// A short description of a frame that is already in a sealed memfd.
    pub fn describe_frame(
        &self,
        frame: &SealedFrame,
        app_name: &str,
        window_title: &str,
        job: Job,
    ) -> Result<ScreenDescription, InferError> {
        let pixels = frame.descriptor()?;
        let summary = self.call(job, |client, ticket| {
            client.describe_screen(
                ticket,
                frame.layout(),
                app_name.to_owned(),
                window_title.to_owned(),
                pixels,
            )
        })?;
        convert::screen(summary)
    }

    fn connect(&self) -> Result<Client, InferError> {
        match &self.socket_dir {
            Some(directory) => Client::connect_in(directory),
            None => Client::connect(),
        }
        .map_err(|error| InferError::Unavailable(format!("crown-context-inferd ({error})").into()))
    }

    /// One request on a fresh connection, waited for until the budget and some slack end.
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        job: Job,
        start: impl FnOnce(
            &mut Client,
            context_protocol::JobTicket,
        ) -> Result<Pending<InferReply<T>>, crownos_ipc::Error>,
    ) -> Result<T, InferError> {
        let ticket = convert::ticket(job);
        let mut client = self.connect()?;
        let lost = |error: crownos_ipc::Error| {
            InferError::Unavailable(format!("crown-context-inferd ({error})").into())
        };
        let pending = start(&mut client, ticket).map_err(lost)?;
        let wait = Duration::from_millis(u64::from(ticket.budget_ms)) + REPLY_SLACK;
        client
            .wait(pending, Some(wait))
            .map_err(lost)?
            .map_err(convert::error)
    }
}

impl ContextTasks for InferdClient {
    fn summarize_segments(
        &self,
        segments: &[SegmentDigest],
        job: Job,
    ) -> Result<Vec<SegmentSummary>, InferError> {
        if segments.is_empty() {
            return Ok(Vec::new());
        }
        let schema = Output::json_for::<SegmentSummaries>().schema_text();
        let max_tokens = SUMMARY_TOKENS_PER_SEGMENT
            .saturating_mul(u32::try_from(segments.len()).unwrap_or(u32::MAX));
        let variables = vec![("segments".to_owned(), summarize_prompt(segments))];
        let text = self.call(job, |client, ticket| {
            client.generate(
                ticket,
                "summarize_segments".into(),
                variables,
                schema,
                max_tokens,
            )
        })?;
        let summaries: SegmentSummaries =
            serde_json::from_str(&text).map_err(|error| InferError::Schema(error.to_string()))?;
        if summaries.segments.len() != segments.len() {
            return Err(InferError::Schema(format!(
                "{} summaries for {} segments",
                summaries.segments.len(),
                segments.len()
            )));
        }
        Ok(summaries.segments)
    }

    /// Copies `screen` once into a sealed memfd; use [`InferdClient::describe_frame`] to
    /// send a frame that is already sealed.
    fn describe_screen(
        &self,
        screen: &Image,
        app_name: &str,
        window_title: &str,
        job: Job,
    ) -> Result<ScreenDescription, InferError> {
        self.describe_frame(&SealedFrame::copy_of(screen)?, app_name, window_title, job)
    }

    /// Copies `image` once into a sealed memfd; use [`InferdClient::recognize_frame`] to
    /// send a frame that is already sealed.
    fn recognize_text(&self, image: &Image, job: Job) -> Result<Vec<TextLine>, InferError> {
        self.recognize_frame(&SealedFrame::copy_of(image)?, job)
    }

    fn embed_text(
        &self,
        texts: Vec<String>,
        purpose: EmbedPurpose,
        job: Job,
    ) -> Result<EmbeddedTexts, InferError> {
        let batch = self.call(job, |client, ticket| {
            client.embed_text(ticket, texts, convert::purpose(purpose))
        })?;
        convert::embedded(batch)
    }

    fn embed_space(&self) -> Option<EmbedSpace> {
        let mut client = self.connect().ok()?;
        let pending = client.capabilities().ok()?;
        let capabilities = client.wait(pending, Some(REPLY_SLACK)).ok()?;
        capabilities.embed_space?.parse().ok()
    }
}
