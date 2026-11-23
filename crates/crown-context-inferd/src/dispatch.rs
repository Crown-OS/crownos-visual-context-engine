use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use context_infer::{
    Capability, ContextTasks, GenRequest, InferError, Media, Output, PROMPT_TEMPLATES,
    PromptTemplate, Sampling, Task,
};
use context_protocol::inference;
use context_protocol::{
    InferCapabilities, InferFailure, InferReply, JobPriority, JobTicket, TaskRoute,
};
use crownos_ipc::{Kind, Message, MethodMsg, PeerId, RemoteError, Server};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::UnboundedSender;

use crate::admission::{Admission, Permit};
use crate::convert;
use crate::engine::Engine;

/// A reply is a few vectors at most; larger embedding batches must be split.
const MAX_VECTOR_BYTES: usize = 768 * 1024;
const MAX_GENERATED_TOKENS: u32 = 2048;
const TASKS: [Task; 6] = [
    Task::RecognizeText,
    Task::EmbedText,
    Task::Rerank,
    Task::SummarizeSegments,
    Task::DescribeScreen,
    Task::Transcribe,
];

/// A finished job, for the serve loop to send.
#[derive(Debug)]
pub(crate) struct Reply {
    peer: PeerId,
    request_id: u32,
    payload: Result<Vec<u8>, RemoteError>,
}

pub(crate) fn send_reply(server: &mut Server, reply: Reply) {
    let sent = match reply.payload {
        Ok(payload) => server.reply(reply.peer, reply.request_id, &payload, Vec::new()),
        Err(error) => server.reply_err(reply.peer, reply.request_id, &error),
    };
    if let Err(error) = sent {
        tracing::debug!(%error, "the peer left before its reply");
    }
}

/// Decodes each request, admits it or answers `Busy`, and runs it on a blocking thread so a
/// slow generation never holds back OCR.
#[derive(Debug)]
pub(crate) struct Dispatcher {
    engine: Arc<Engine>,
    replies: UnboundedSender<Reply>,
    fixed_cost: Admission,
    generation: Admission,
}

impl Dispatcher {
    pub(crate) fn new(engine: Arc<Engine>, replies: UnboundedSender<Reply>) -> Self {
        Self {
            engine,
            replies,
            fixed_cost: Admission::new(6, 2, Duration::from_millis(100)),
            generation: Admission::new(1, 1, Duration::from_secs(2)),
        }
    }

    pub(crate) fn dispatch(
        &self,
        server: &mut Server,
        peer: PeerId,
        message: Message,
    ) -> Result<bool, crownos_ipc::Error> {
        if message.kind != Kind::Request {
            server.drop_peer(peer);
            return Ok(true);
        }
        let selector = message.selector;
        let fixed = Some(&self.fixed_cost);
        let generation = Some(&self.generation);
        if selector == inference::capabilities::SELECTOR {
            self.run(
                server,
                peer,
                message,
                None,
                |engine, _: inference::capabilities, _| capabilities(engine),
            );
        } else if selector == inference::unload::SELECTOR {
            self.run(
                server,
                peer,
                message,
                None,
                |engine, _: inference::unload, _| {
                    u32::try_from(engine.router.unload_all()).unwrap_or(u32::MAX)
                },
            );
        } else if selector == inference::ocr::SELECTOR {
            self.run(
                server,
                peer,
                message,
                fixed,
                |engine, call: inference::ocr, fds| {
                    let image = convert::image(call.image, first(&fds)?)?;
                    let lines = engine.router.recognize_text(image, convert::job(call.job));
                    lines.map(convert::lines).map_err(convert::failure)
                },
            );
        } else if selector == inference::embed_text::SELECTOR {
            self.run(
                server,
                peer,
                message,
                fixed,
                |engine, call: inference::embed_text, _| embed_text(engine, call),
            );
        } else if selector == inference::rerank::SELECTOR {
            self.run(
                server,
                peer,
                message,
                fixed,
                |engine, call: inference::rerank, _| {
                    let job = convert::job(call.job);
                    let scores = engine.router.rerank(call.query, call.documents, job);
                    scores.map_err(convert::failure)
                },
            );
        } else if selector == inference::generate::SELECTOR {
            self.run(
                server,
                peer,
                message,
                generation,
                |engine, call: inference::generate, _| {
                    let template =
                        Template::new(call.template, call.vars, call.schema, call.max_tokens);
                    generate(engine, call.job, &template, None)
                },
            );
        } else if selector == inference::generate_with_image::SELECTOR {
            self.run(
                server,
                peer,
                message,
                generation,
                |engine, call: inference::generate_with_image, fds| {
                    let image = convert::image(call.image, first(&fds)?)?;
                    let template =
                        Template::new(call.template, call.vars, call.schema, call.max_tokens);
                    generate(engine, call.job, &template, Some(image))
                },
            );
        } else if selector == inference::describe_screen::SELECTOR {
            self.run(
                server,
                peer,
                message,
                generation,
                |engine, call: inference::describe_screen, fds| {
                    let image = convert::image(call.image, first(&fds)?)?;
                    let job = convert::job(call.job);
                    let description = engine.tasks.describe_screen(
                        &image,
                        &call.app_name,
                        &call.window_title,
                        job,
                    );
                    description.map(convert::screen).map_err(convert::failure)
                },
            );
        } else {
            let _ = server.reply_err(peer, message.req_id, &RemoteError::unknown_method(selector));
        }
        Ok(true)
    }

    /// Decodes `M`, takes a place in `admission` (or answers `Busy`), then runs `work` on a
    /// blocking thread and queues its reply.
    fn run<M, W>(
        &self,
        server: &mut Server,
        peer: PeerId,
        message: Message,
        admission: Option<&Admission>,
        work: W,
    ) where
        M: MethodMsg<Args = M> + JobMethod + DeserializeOwned + Send + 'static,
        M::Reply: Serialize + FromFailure + Send + 'static,
        W: FnOnce(&Engine, M, Vec<OwnedFd>) -> M::Reply + Send + 'static,
    {
        let Message {
            req_id: request_id,
            payload,
            fds,
            ..
        } = message;
        let call: M = match crownos_ipc::postcard::from_bytes(&payload) {
            Ok(call) if fds.len() == M::FD_COUNT => call,
            _ => {
                server.drop_peer(peer);
                return;
            }
        };
        let ticket = call.ticket();
        let interactive = ticket.is_some_and(|ticket| ticket.priority == JobPriority::Interactive);
        let admitted = admission.map(|admission| admission.admit(interactive));
        let permit: Option<Permit> = match admitted.transpose() {
            Ok(permit) => permit,
            Err(retry_after) => {
                let busy = M::Reply::from_failure(InferFailure::Busy {
                    retry_after_ms: u32::try_from(retry_after.as_millis()).unwrap_or(u32::MAX),
                });
                send_reply(server, reply(peer, request_id, &busy));
                return;
            }
        };
        let engine = Arc::clone(&self.engine);
        let replies = self.replies.clone();
        if let Some(ticket) = ticket {
            tracing::debug!(job_id = ticket.job_id, priority = ?ticket.priority, "job admitted");
        }
        tokio::task::spawn_blocking(move || {
            let answer = work(&engine, call, fds);
            drop(permit);
            if let Some(ticket) = ticket {
                tracing::debug!(job_id = ticket.job_id, "job done");
            }
            let _ = replies.send(reply(peer, request_id, &answer));
        });
    }
}

fn reply(peer: PeerId, request_id: u32, answer: &impl Serialize) -> Reply {
    Reply {
        peer,
        request_id,
        payload: crownos_ipc::postcard::to_stdvec(answer)
            .map_err(|_| RemoteError::handler("reply serialization failed")),
    }
}

/// The ticket a request carries, if it is a job.
pub(crate) trait JobMethod {
    fn ticket(&self) -> Option<JobTicket>;
}

macro_rules! job_methods {
    ($($method:ident),*) => {
        $(impl JobMethod for inference::$method {
            fn ticket(&self) -> Option<JobTicket> {
                Some(self.job)
            }
        })*
    };
}

job_methods!(
    ocr,
    embed_text,
    rerank,
    generate,
    generate_with_image,
    describe_screen
);

impl JobMethod for inference::capabilities {
    fn ticket(&self) -> Option<JobTicket> {
        None
    }
}

impl JobMethod for inference::unload {
    fn ticket(&self) -> Option<JobTicket> {
        None
    }
}

/// A reply type that can carry a refusal, so a full queue answers in the method's own type.
pub(crate) trait FromFailure {
    fn from_failure(failure: InferFailure) -> Self;
}

impl<T> FromFailure for InferReply<T> {
    fn from_failure(failure: InferFailure) -> Self {
        Err(failure)
    }
}

impl FromFailure for InferCapabilities {
    fn from_failure(_: InferFailure) -> Self {
        Self::default()
    }
}

impl FromFailure for u32 {
    fn from_failure(_: InferFailure) -> Self {
        0
    }
}

fn first(fds: &[OwnedFd]) -> Result<&OwnedFd, InferFailure> {
    fds.first()
        .ok_or_else(|| InferFailure::InvalidInput("no memfd with the image".into()))
}

fn capabilities(engine: &Engine) -> InferCapabilities {
    let router = &engine.router;
    let mut devices: Vec<String> = router
        .available_devices()
        .map(|(runtime, device)| format!("{runtime:?}/{device:?}"))
        .collect();
    devices.sort_unstable();
    InferCapabilities {
        devices,
        routes: TASKS
            .into_iter()
            .map(|task| TaskRoute {
                task: format!("{task:?}"),
                candidates: router
                    .routes(task)
                    .iter()
                    .map(|candidate| {
                        format!(
                            "{}@{:?}/{:?}",
                            candidate.model, candidate.runtime, candidate.device
                        )
                    })
                    .collect(),
            })
            .collect(),
        loaded: router
            .loaded_models()
            .into_iter()
            .map(|(capability, model, runtime, device)| {
                format!("{capability:?} {model}@{runtime:?}/{device:?}")
            })
            .collect(),
        embed_space: router.embed_space().map(|space| space.to_string()),
        templates: PROMPT_TEMPLATES
            .iter()
            .map(|template| template.id.to_owned())
            .collect(),
    }
}

fn embed_text(
    engine: &Engine,
    call: inference::embed_text,
) -> InferReply<context_protocol::EmbeddedBatch> {
    let dimensions = engine
        .router
        .embed_space()
        .map_or(0, |space| space.dimensions as usize);
    if call.texts.len() * dimensions * size_of::<f32>() > MAX_VECTOR_BYTES {
        return Err(InferFailure::InvalidInput(format!(
            "{} texts give more than {MAX_VECTOR_BYTES} bytes of vectors; split the batch",
            call.texts.len()
        )));
    }
    let job = convert::job(call.job);
    let embedded = engine
        .router
        .embed(call.texts, convert::purpose(call.purpose), job);
    embedded.map(convert::embedded).map_err(convert::failure)
}

/// What a caller sends to fill a prompt template.
struct Template {
    id: String,
    variables: BTreeMap<String, String>,
    schema: Option<String>,
    max_tokens: u32,
}

impl Template {
    fn new(
        id: String,
        variables: Vec<(String, String)>,
        schema: Option<String>,
        max_tokens: u32,
    ) -> Self {
        Self {
            id,
            variables: variables.into_iter().collect(),
            schema,
            max_tokens,
        }
    }
}

fn generate(
    engine: &Engine,
    ticket: JobTicket,
    template: &Template,
    image: Option<context_infer::Image>,
) -> InferReply<String> {
    let request = || -> Result<(Task, GenRequest), InferError> {
        let prompt = PromptTemplate::find(&template.id)?;
        let output = match &template.schema {
            Some(schema) => Output::json_from_text(schema)?,
            None => Output::Text,
        };
        let request = GenRequest {
            messages: prompt.render(&template.variables)?,
            media: image.map(Media::Image).into_iter().collect(),
            output,
            sampling: Sampling::default(),
            max_tokens: template.max_tokens.min(MAX_GENERATED_TOKENS),
            job: convert::job(ticket),
        };
        Ok((prompt.task, request))
    };
    let (task, request) = request().map_err(convert::failure)?;
    engine
        .router
        .generate(task, request)
        .map(|generation| generation.text)
        .map_err(convert::failure)
}

/// OCR and embedding stay loaded; the rest unloads when idle.
pub(crate) const RESIDENT: [Capability; 2] = [Capability::Ocr, Capability::Embed];
