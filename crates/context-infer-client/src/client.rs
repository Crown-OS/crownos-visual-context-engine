use std::path::PathBuf;
use std::time::{Duration, Instant};

use context_infer::{
    ContextTasks, EmbedPurpose, EmbedSpace, EmbeddedTexts, Image, InferError, Job, Output,
    ScreenDescription, SegmentDigest, SegmentSummaries, SegmentSummary, TextLine, summarize_prompt,
};
use context_protocol::InferReply;
use context_protocol::inference::Client;
use crownos_ipc::Pending;
use rustix::event::{PollFd, PollFlags, Timespec};

use crate::convert;
use crate::frame::SealedFrame;

/// How often a wait for inferd looks at the job's cancel token.
const CANCEL_CHECK: Duration = Duration::from_millis(20);
/// Time past the job budget that the client waits for inferd's own `Timeout`.
const REPLY_SLACK: Duration = Duration::from_secs(5);
const SUMMARY_TOKENS_PER_SEGMENT: u32 = 400;
/// inferd answers at most 2048 tokens per request.
const SEGMENTS_PER_REQUEST: usize = 5;
/// inferd answers at most 768 KiB of vectors per request.
const MAX_VECTOR_BYTES: usize = 768 * 1024;
/// Small enough for vectors up to 4096 wide; later requests use the width of the first answer.
const FIRST_EMBED_BATCH: usize = 48;

/// Waits until the connection can be read, or `timeout` passes.
fn wait_readable(client: &Client, timeout: Duration) -> Result<(), crownos_ipc::Error> {
    let mut descriptors = [PollFd::new(client, PollFlags::IN)];
    let timeout = Timespec::try_from(timeout).ok();
    match rustix::event::poll(&mut descriptors, timeout.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => Err(crownos_ipc::Error::Io(error.into())),
    }
}

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
        job: &Job,
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
        job: &Job,
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

    /// One request on a fresh connection, waited for until the budget and some slack end. A
    /// cancelled job tells inferd to end it, then closes the connection.
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        job: &Job,
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
        let deadline =
            Instant::now() + Duration::from_millis(u64::from(ticket.budget_ms)) + REPLY_SLACK;
        loop {
            if let Some(answer) = client.take(&pending).map_err(lost)? {
                return answer.map_err(convert::error);
            }
            if job.cancel.is_cancelled() {
                let _ = client.cancel(ticket.job_id);
                return Err(InferError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(lost(crownos_ipc::Error::Timeout));
            }
            wait_readable(&client, left.min(CANCEL_CHECK)).map_err(lost)?;
            client.handle_writable().map_err(lost)?;
            client.handle_readable().map_err(lost)?;
        }
    }
}

impl InferdClient {
    /// One `summarize_segments` request; inferd answers at most 2048 tokens.
    fn summarize_request(
        &self,
        segments: &[SegmentDigest],
        job: &Job,
    ) -> Result<Vec<SegmentSummary>, InferError> {
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
}

impl ContextTasks for InferdClient {
    fn summarize_segments(
        &self,
        segments: &[SegmentDigest],
        job: Job,
    ) -> Result<Vec<SegmentSummary>, InferError> {
        let mut summaries = Vec::with_capacity(segments.len());
        for request in segments.chunks(SEGMENTS_PER_REQUEST) {
            summaries.extend(self.summarize_request(request, &job)?);
        }
        Ok(summaries)
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
        self.describe_frame(&SealedFrame::copy_of(screen)?, app_name, window_title, &job)
    }

    /// Copies `image` once into a sealed memfd; use [`InferdClient::recognize_frame`] to
    /// send a frame that is already sealed.
    fn recognize_text(&self, image: &Image, job: Job) -> Result<Vec<TextLine>, InferError> {
        self.recognize_frame(&SealedFrame::copy_of(image)?, &job)
    }

    fn embed_text(
        &self,
        texts: Vec<String>,
        purpose: EmbedPurpose,
        job: Job,
    ) -> Result<EmbeddedTexts, InferError> {
        let mut embedded: Option<EmbeddedTexts> = None;
        let mut remaining = texts.as_slice();
        let mut batch_size = FIRST_EMBED_BATCH;
        loop {
            let (request, rest) = remaining.split_at(batch_size.min(remaining.len()));
            let answer = self.call(&job, |client, ticket| {
                client.embed_text(ticket, request.to_vec(), convert::purpose(purpose))
            })?;
            let answer = convert::embedded(answer)?;
            let row_bytes = answer.embeddings.dimensions.max(1) * size_of::<f32>();
            batch_size = (MAX_VECTOR_BYTES / row_bytes).max(1);
            embedded = Some(match embedded {
                None => answer,
                Some(mut so_far) if so_far.space == answer.space => {
                    so_far.embeddings.values.extend(answer.embeddings.values);
                    so_far
                }
                Some(so_far) => {
                    return Err(InferError::Backend(format!(
                        "the embedding space changed from {} to {} within one call",
                        so_far.space, answer.space
                    )));
                }
            });
            remaining = rest;
            if remaining.is_empty() {
                break;
            }
        }
        embedded.ok_or_else(|| InferError::Backend("no embedding answer".into()))
    }

    fn embed_space(&self) -> Option<EmbedSpace> {
        let mut client = self.connect().ok()?;
        let pending = client.capabilities().ok()?;
        let capabilities = client.wait(pending, Some(REPLY_SLACK)).ok()?;
        capabilities.embed_space?.parse().ok()
    }
}
