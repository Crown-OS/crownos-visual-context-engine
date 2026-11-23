use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use context_infer::registry::Artifact;
use context_infer::{
    Device, GenRequest, Generate, GenerateCapabilities, Generation, InferError, LoadSpec, Media,
    Role,
};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::mtmd::{
    MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText, mtmd_default_marker,
};

use crate::grammar::sampler;
use crate::model::{background_threads, load_model};
use crate::util::error::backend_error;
use crate::util::utf8::Utf8Joiner;

const CONTEXT_TOKENS: u32 = 8192;
const BATCH_TOKENS: u32 = 512;
/// llama.cpp reads the logits of the last decoded token at this index.
const LAST_LOGITS: i32 = -1;

/// A GGUF language model, with its vision projector when the registry names one.
#[derive(Debug)]
pub struct GgufGenerator {
    backend: Arc<LlamaBackend>,
    model: LlamaModel,
    projector: Option<Mutex<MtmdContext>>,
    template: LlamaChatTemplate,
    context_tokens: u32,
}

impl GgufGenerator {
    pub fn load(backend: Arc<LlamaBackend>, spec: &LoadSpec<'_>) -> Result<Self, InferError> {
        let Artifact::Gguf { model, mmproj, .. } = spec.artifact else {
            return Err(InferError::Unavailable(
                "a GGUF artifact for generation".into(),
            ));
        };
        let model = load_model(&backend, &spec.models_dir.join(&model.path), spec.device)?;
        let projector = match mmproj {
            Some(file) => {
                let path = spec.models_dir.join(&file.path);
                let path = path.to_str().ok_or_else(|| {
                    InferError::Unavailable(format!("the non-UTF-8 path {}", path.display()).into())
                })?;
                let params = MtmdContextParams {
                    use_gpu: spec.device != Device::Cpu,
                    n_threads: background_threads(),
                    ..MtmdContextParams::default()
                };
                let projector = MtmdContext::init_from_file(path, &model, &params)
                    .map_err(|error| backend_error("loading the vision projector", error))?;
                Some(Mutex::new(projector))
            }
            None => None,
        };
        let template = model
            .chat_template(None)
            .map_err(|error| backend_error("reading the chat template", error))?;
        Ok(Self {
            context_tokens: model.n_ctx_train().clamp(1, CONTEXT_TOKENS),
            backend,
            model,
            projector,
            template,
        })
    }

    /// The prompt in the model's chat format, with one media marker per image before the
    /// last user message.
    fn prompt(&self, request: &GenRequest, images: usize) -> Result<String, InferError> {
        let last_user = request
            .messages
            .iter()
            .rposition(|message| message.role == Role::User);
        let markers = mtmd_default_marker().repeat(images);
        let messages = request
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let role = match message.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let content = if Some(index) == last_user {
                    format!("{markers}{}", message.content)
                } else {
                    message.content.to_string()
                };
                LlamaChatMessage::new(role.to_owned(), content)
                    .map_err(|error| backend_error("building the chat", error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.model
            .apply_chat_template(&self.template, &messages, true)
            .map_err(|error| backend_error("applying the chat template", error))
    }

    /// Decodes the prompt and gives the next position.
    fn eval_prompt(
        &self,
        context: &mut LlamaContext<'_>,
        prompt: String,
        media: &[Media],
        budget: u32,
    ) -> Result<i32, InferError> {
        if media.is_empty() {
            let tokens = self.model.vocab().tokenize(prompt.as_bytes(), true, true);
            self.check_fits(tokens.len(), budget)?;
            let mut batch = LlamaBatch::new(BATCH_TOKENS as usize, 1);
            let mut position = 0;
            for chunk in tokens.chunks(BATCH_TOKENS as usize) {
                batch.clear();
                for (offset, &token) in chunk.iter().enumerate() {
                    let is_last = position + offset + 1 == tokens.len();
                    let at = i32::try_from(position + offset).unwrap_or(i32::MAX);
                    batch
                        .add(token, at, &[0], is_last)
                        .map_err(|error| backend_error("filling the batch", error))?;
                }
                context
                    .decode(&mut batch)
                    .map_err(|error| backend_error("decoding the prompt", error))?;
                position += chunk.len();
            }
            return Ok(i32::try_from(position).unwrap_or(i32::MAX));
        }
        let projector = self.projector.as_ref().ok_or_else(|| {
            InferError::Unavailable("images for a model without a projector".into())
        })?;
        let mut projector = projector.lock().unwrap_or_else(PoisonError::into_inner);
        let bitmaps = media
            .iter()
            .map(|media| bitmap(&projector, media))
            .collect::<Result<Vec<_>, _>>()?;
        let bitmap_references: Vec<&MtmdBitmap> = bitmaps.iter().collect();
        let chunks = projector
            .tokenize(
                MtmdInputText {
                    text: prompt,
                    add_special: true,
                    parse_special: true,
                },
                &bitmap_references,
            )
            .map_err(|error| backend_error("tokenizing the prompt and images", error))?;
        self.check_fits(chunks.total_tokens(), budget)?;
        chunks
            .eval_chunks(&mut projector, context, 0, 0, BATCH_TOKENS as i32, true)
            .map_err(|error| backend_error("decoding the prompt and images", error))
    }

    fn check_fits(&self, prompt_tokens: usize, budget: u32) -> Result<(), InferError> {
        if prompt_tokens + budget as usize > self.context_tokens as usize {
            return Err(InferError::Unavailable(
                format!(
                    "room for {prompt_tokens} prompt and {budget} answer tokens in a context of {}",
                    self.context_tokens
                )
                .into(),
            ));
        }
        Ok(())
    }
}

/// Raw pixels go to the projector in one packed RGB copy; encoded images are decoded there.
fn bitmap(projector: &MtmdContext, media: &Media) -> Result<MtmdBitmap, InferError> {
    match media {
        Media::Image(image) => {
            MtmdBitmap::from_image_data(image.width(), image.height(), &image.to_rgb8())
                .map_err(|error| backend_error("passing the image", error))
        }
        Media::Encoded { bytes, .. } => MtmdBitmap::from_buffer(projector, bytes.as_slice(), false)
            .map_err(|error| backend_error("decoding the image", error)),
        Media::Pcm16k(_) => Err(InferError::Unavailable("audio input".into())),
    }
}

impl Generate for GgufGenerator {
    fn capabilities(&self) -> GenerateCapabilities {
        GenerateCapabilities {
            images: self.projector.is_some(),
            json_schema: true,
            context_tokens: self.context_tokens,
        }
    }

    fn stream(
        &self,
        request: &GenRequest,
        on_text: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<Generation, InferError> {
        let prompt = self.prompt(request, request.media.len())?;
        let threads = background_threads();
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.context_tokens))
            .with_n_batch(BATCH_TOKENS)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut context = self
            .model
            .new_context(&self.backend, params)
            .map_err(|error| backend_error("creating the context", error))?;
        let mut position =
            self.eval_prompt(&mut context, prompt, &request.media, request.max_tokens)?;
        let prompt_tokens = u32::try_from(position).unwrap_or(u32::MAX);
        let mut sampler = sampler(&self.model, &request.output, request.sampling)?;
        let vocab = self.model.vocab();
        let mut batch = LlamaBatch::new(1, 1);
        let mut joiner = Utf8Joiner::default();
        let mut generation = Generation {
            prompt_tokens,
            ..Generation::default()
        };
        while generation.generated_tokens < request.max_tokens {
            if Instant::now() >= request.job.deadline {
                return Err(InferError::Timeout);
            }
            let token = sampler.sample(&context, LAST_LOGITS);
            if vocab.is_eog(token) {
                break;
            }
            let piece = joiner.push(&vocab.token_to_piece(token, false, None));
            generation.text.push_str(&piece);
            generation.generated_tokens += 1;
            if on_text(&piece).is_break() {
                break;
            }
            batch.clear();
            batch
                .add(token, position, &[0], true)
                .map_err(|error| backend_error("filling the batch", error))?;
            context
                .decode(&mut batch)
                .map_err(|error| backend_error("decoding", error))?;
            position += 1;
        }
        Ok(generation)
    }
}
