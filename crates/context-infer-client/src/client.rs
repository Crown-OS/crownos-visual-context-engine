use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use context_infer::{
    ContextTasks, EmbedPurpose, EmbedSpace, EmbeddedTexts, Image, InferError, Job, Output,
    ScreenDescription, SegmentDigest, SegmentSummaries, SegmentSummary, TextLine, summarize_prompt,
};
use context_protocol::inference::Client;
use context_protocol::{InferReply, JobTicket};
use crownos_ipc::Pending;

use crate::connection::{Command, Poller, SharedConnection};
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

/// Talks to crown-context-inferd over one shared connection. Calls from many threads go out
/// together on it; inferd orders them.
#[derive(Debug, Clone)]
pub struct InferdClient {
    connection: SharedConnection,
}

/// What [`InferdClient::exchange`] starts: one request on the connection.
type Start<T> =
    Box<dyn FnOnce(&mut Client) -> Result<Pending<T>, crownos_ipc::Error> + Send + 'static>;

impl InferdClient {
    /// Connects now to the socket in `socket_dir` (default `$XDG_RUNTIME_DIR/crownos`), and
    /// again the same way after the connection is lost.
    pub fn new(socket_dir: Option<PathBuf>) -> Self {
        Self {
            connection: SharedConnection::open(Box::new(move || connect(socket_dir.as_deref()))),
        }
    }

    /// OCR of a frame that is already in a sealed memfd.
    pub fn recognize_frame(
        &self,
        frame: &SealedFrame,
        job: &Job,
    ) -> Result<Vec<TextLine>, InferError> {
        let (layout, pixels) = (frame.layout(), frame.descriptor()?);
        let lines = self.call(job, move |client, ticket| {
            client.ocr(ticket, layout, pixels)
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
        let (layout, pixels) = (frame.layout(), frame.descriptor()?);
        let (app_name, window_title) = (app_name.to_owned(), window_title.to_owned());
        let summary = self.call(job, move |client, ticket| {
            client.describe_screen(ticket, layout, app_name, window_title, pixels)
        })?;
        convert::screen(summary)
    }

    /// One job on the shared connection, waited for until the budget and some slack end. A
    /// cancelled job tells inferd to end it.
    fn call<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        job: &Job,
        start: impl FnOnce(&mut Client, JobTicket) -> Result<Pending<InferReply<T>>, crownos_ipc::Error>
        + Send
        + 'static,
    ) -> Result<T, InferError> {
        let ticket = convert::ticket(job);
        let wait = Duration::from_millis(u64::from(ticket.budget_ms)) + REPLY_SLACK;
        let start: Start<InferReply<T>> = Box::new(move |client| start(client, ticket));
        let cancelled = || job.cancel.is_cancelled();
        match self.exchange(wait, cancelled, start) {
            Err(InferError::Cancelled) => {
                self.connection.send(Command::Cancel(ticket.job_id));
                Err(InferError::Cancelled)
            }
            answer => answer?.map_err(convert::error),
        }
    }

    /// Sends a request on the shared connection and waits up to `wait` for its reply, or until
    /// `cancelled` says so.
    fn exchange<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        wait: Duration,
        cancelled: impl Fn() -> bool,
        start: Start<T>,
    ) -> Result<T, InferError> {
        let (answer, answered) = mpsc::channel();
        let begin = Box::new(move |client: Option<&mut Client>| -> Option<Poller> {
            let Some(client) = client else {
                let _ = answer.send(Err(crownos_ipc::Error::Disconnected));
                return None;
            };
            let pending = match start(client) {
                Ok(pending) => pending,
                Err(error) => {
                    let _ = answer.send(Err(error));
                    return None;
                }
            };
            Some(Box::new(move |client: &mut Client| {
                match client.take(&pending) {
                    Ok(None) => return false,
                    Ok(Some(reply)) => {
                        let _ = answer.send(Ok(reply));
                    }
                    Err(error) => {
                        let _ = answer.send(Err(error));
                    }
                }
                true
            }))
        });
        if !self.connection.send(Command::Begin(begin)) {
            return Err(lost(&crownos_ipc::Error::Disconnected));
        }
        let deadline = Instant::now() + wait;
        loop {
            match answered.recv_timeout(CANCEL_CHECK) {
                Ok(reply) => return reply.map_err(|error| lost(&error)),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(lost(&crownos_ipc::Error::Disconnected));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if cancelled() {
                return Err(InferError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(lost(&crownos_ipc::Error::Timeout));
            }
        }
    }
}

fn connect(socket_dir: Option<&std::path::Path>) -> Result<Client, InferError> {
    match socket_dir {
        Some(directory) => Client::connect_in(directory),
        None => Client::connect(),
    }
    .map_err(|error| lost(&error))
}

fn lost(error: &crownos_ipc::Error) -> InferError {
    InferError::Unavailable(format!("crown-context-inferd ({error})").into())
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
        let text = self.call(job, move |client, ticket| {
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
            let texts = request.to_vec();
            let answer = self.call(&job, move |client, ticket| {
                client.embed_text(ticket, texts, convert::purpose(purpose))
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
        let start: Start<_> = Box::new(|client| client.capabilities());
        let capabilities = self.exchange(REPLY_SLACK, || false, start).ok()?;
        capabilities.embed_space?.parse().ok()
    }
}
