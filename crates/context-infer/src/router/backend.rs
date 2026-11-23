use std::path::Path;
use std::sync::Arc;

use crownos_config::system::ai::{Device, Runtime};

use crate::capability::{Embed, EmbedSpace, Generate, Ocr, Rerank, Transcribe};
use crate::error::InferError;
use crate::registry::{Artifact, Candidate, ModelEntry, Registry};

/// Everything a backend needs to load one model on one device.
#[derive(Clone, Copy, Debug)]
pub struct LoadSpec<'a> {
    pub model_id: &'a str,
    pub model: &'a ModelEntry,
    pub artifact: &'a Artifact,
    pub device: Device,
    pub models_dir: &'a Path,
}

impl<'a> LoadSpec<'a> {
    pub(crate) fn new(
        registry: &'a Registry,
        candidate: &'a Candidate,
        models_dir: &'a Path,
    ) -> Result<Self, InferError> {
        let unavailable = || InferError::Unavailable(format!("model {}", candidate.model).into());
        let model = registry
            .models
            .get(&candidate.model)
            .ok_or_else(unavailable)?;
        let artifact = model
            .artifact_for(candidate.runtime)
            .ok_or_else(unavailable)?;
        Ok(Self {
            model_id: &candidate.model,
            model,
            artifact,
            device: candidate.device,
            models_dir,
        })
    }

    pub fn embed_space(&self) -> Option<EmbedSpace> {
        Some(EmbedSpace {
            model: self.model_id.to_owned(),
            dimensions: self.model.embedding_dimensions?,
        })
    }

    fn unsupported(&self, capability: &str) -> InferError {
        InferError::Unavailable(format!("{capability} for {}", self.model_id).into())
    }
}

/// One inference runtime, as a backend crate wraps it. Each load method gives a model that
/// stays loaded until the router drops it.
pub trait RuntimeBackend: Send + Sync {
    fn runtime(&self) -> Runtime;

    /// The runtime build and driver versions; a cached benchmark is valid only for these.
    fn version(&self) -> String;

    /// A cheap check that `device` works. It must not load a model.
    fn probe(&self, device: Device) -> bool;

    fn load_generator(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Generate>, InferError> {
        Err(spec.unsupported("generation"))
    }

    fn load_embedder(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Embed>, InferError> {
        Err(spec.unsupported("embedding"))
    }

    fn load_reranker(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Rerank>, InferError> {
        Err(spec.unsupported("reranking"))
    }

    fn load_ocr(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Ocr>, InferError> {
        Err(spec.unsupported("text recognition"))
    }

    fn load_transcriber(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Transcribe>, InferError> {
        Err(spec.unsupported("transcription"))
    }
}
