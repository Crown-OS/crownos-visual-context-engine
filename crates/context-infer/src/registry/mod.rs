//! The model catalog: which models exist, what each one can do, which files each runtime
//! loads, and the ranked candidates for each [`Task`]. Swapping a model is an edit here.

mod load;

use std::collections::BTreeMap;
use std::path::PathBuf;

use crownos_config::system::ai::{Device, Runtime};
use serde::{Deserialize, Serialize};

use crate::capability::EmbedSpace;
use crate::task::Task;

pub use load::{RegistryError, SYSTEM_REGISTRY_PATH, user_override_path};

pub const REGISTRY_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub models: BTreeMap<String, ModelEntry>,
    pub tasks: BTreeMap<Task, Vec<Candidate>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    Generate,
    Vision,
    JsonOutput,
    Embed,
    Rerank,
    Ocr,
    Transcribe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub capabilities: Vec<Capability>,
    pub licence: String,
    /// Output size of an embedding model, after any Matryoshka truncation.
    #[serde(default)]
    pub embedding_dimensions: Option<u32>,
    /// How an embedding graph without a pooled output turns token states into one vector.
    #[serde(default)]
    pub pooling: Option<Pooling>,
    /// Put before each query or document text, as the model card asks.
    #[serde(default)]
    pub query_prefix: String,
    #[serde(default)]
    pub document_prefix: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pooling {
    Cls,
    Mean,
    LastToken,
}

/// The files one runtime loads. Paths are relative to the models directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Artifact {
    Gguf {
        model: ModelFile,
        #[serde(default)]
        mmproj: Option<ModelFile>,
        quant: String,
    },
    /// Graphs by role (for OCR: `det` and `rec`) and the files they need (`dict`).
    Onnx {
        graphs: BTreeMap<String, OnnxGraph>,
        #[serde(default)]
        files: BTreeMap<String, ModelFile>,
        /// For example `fp32` or `int8`.
        #[serde(default)]
        quant: String,
    },
    Remote {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFile {
    pub path: PathBuf,
    /// Empty until the file is pinned.
    #[serde(default)]
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnnxGraph {
    pub path: PathBuf,
    #[serde(default)]
    pub sha256: String,
    /// Input name to static shape; `-1` marks a dynamic axis.
    #[serde(default)]
    pub shapes: BTreeMap<String, Vec<i64>>,
}

/// One way to run a task: a model on a runtime and device, with the conditions it needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub model: String,
    pub runtime: Runtime,
    pub device: Device,
    #[serde(default = "allowed")]
    pub on_battery: bool,
    #[serde(default)]
    pub min_tier: Tier,
}

const fn allowed() -> bool {
    true
}

/// How much memory and power the machine gives to inference.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Tier {
    #[default]
    Low,
    Mid,
    High,
}

impl Artifact {
    pub const fn runtime(&self) -> Runtime {
        match self {
            Self::Gguf { .. } => Runtime::LlamaCpp,
            Self::Onnx { .. } => Runtime::OnnxRuntime,
            Self::Remote { .. } => Runtime::OpenAiCompatible,
        }
    }

    pub fn quantization(&self) -> &str {
        match self {
            Self::Gguf { quant, .. } | Self::Onnx { quant, .. } => quant,
            Self::Remote { .. } => "remote",
        }
    }
}

impl ModelEntry {
    pub fn artifact_for(&self, runtime: Runtime) -> Option<&Artifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.runtime() == runtime)
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

impl Registry {
    pub fn candidates(&self, task: Task) -> &[Candidate] {
        self.tasks.get(&task).map_or(&[], Vec::as_slice)
    }

    pub fn embed_space(&self, candidate: &Candidate) -> Option<EmbedSpace> {
        let model = self.models.get(&candidate.model)?;
        Some(EmbedSpace {
            model: candidate.model.clone(),
            runtime: candidate.runtime,
            quantization: model
                .artifact_for(candidate.runtime)?
                .quantization()
                .to_owned(),
            dimensions: model.embedding_dimensions?,
        })
    }
}

#[cfg(test)]
mod tests;
