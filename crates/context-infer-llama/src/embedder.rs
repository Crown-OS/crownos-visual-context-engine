use std::num::NonZeroU32;
use std::sync::Arc;

use context_infer::registry::Artifact;
use context_infer::{Embed, EmbedPurpose, EmbedSpace, Embeddings, InferError, LoadSpec};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;

use crate::model::{background_threads, load_model};
use crate::util::error::backend_error;

const MAX_TOKENS: usize = 512;

/// A GGUF embedding model; the pooling comes from the file.
#[derive(Debug)]
pub struct GgufEmbedder {
    backend: Arc<LlamaBackend>,
    model: LlamaModel,
    space: EmbedSpace,
    query_prefix: String,
    document_prefix: String,
}

impl GgufEmbedder {
    pub fn load(backend: Arc<LlamaBackend>, spec: &LoadSpec<'_>) -> Result<Self, InferError> {
        let Artifact::Gguf { model, .. } = spec.artifact else {
            return Err(InferError::Unavailable(
                "a GGUF artifact for embedding".into(),
            ));
        };
        let space = spec.embed_space().ok_or_else(|| {
            InferError::Unavailable(format!("the dimensions of {}", spec.model_id).into())
        })?;
        Ok(Self {
            model: load_model(&backend, &spec.models_dir.join(&model.path), spec.device)?,
            backend,
            space,
            query_prefix: spec.model.query_prefix.clone(),
            document_prefix: spec.model.document_prefix.clone(),
        })
    }
}

impl Embed for GgufEmbedder {
    fn space(&self) -> &EmbedSpace {
        &self.space
    }

    fn embed(&self, texts: &[String], purpose: EmbedPurpose) -> Result<Embeddings, InferError> {
        let dimensions = self.space.dimensions as usize;
        let prefix = match purpose {
            EmbedPurpose::Query => &self.query_prefix,
            EmbedPurpose::Document => &self.document_prefix,
        };
        let batch_tokens = u32::try_from(MAX_TOKENS).unwrap_or(u32::MAX);
        let params = LlamaContextParams::default()
            .with_embeddings(true)
            .with_n_ctx(NonZeroU32::new(batch_tokens))
            .with_n_batch(batch_tokens)
            .with_n_ubatch(batch_tokens)
            .with_n_threads(background_threads())
            .with_n_threads_batch(background_threads());
        let mut context = self
            .model
            .new_context(&self.backend, params)
            .map_err(|error| backend_error("creating the embedding context", error))?;
        let mut batch = LlamaBatch::new(MAX_TOKENS, 1);
        let mut embeddings = Embeddings {
            dimensions,
            values: Vec::with_capacity(texts.len() * dimensions),
        };
        for text in texts {
            let mut tokens =
                self.model
                    .vocab()
                    .tokenize(format!("{prefix}{text}").as_bytes(), true, true);
            tokens.truncate(MAX_TOKENS);
            context.clear_kv_cache();
            batch.clear();
            batch
                .add_sequence(&tokens, 0, true)
                .map_err(|error| backend_error("filling the batch", error))?;
            context
                .decode(&mut batch)
                .map_err(|error| backend_error("encoding", error))?;
            let vector = context
                .embeddings_seq_ith(0)
                .map_err(|error| backend_error("reading the embedding", error))?;
            embeddings.push_normalized(vector);
        }
        Ok(embeddings)
    }
}
