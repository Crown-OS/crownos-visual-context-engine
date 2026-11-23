use std::sync::Arc;

use context_infer::{Device, Embed, InferError, LoadSpec, Ocr, Runtime, RuntimeBackend};
use crownos_config::system::ai::AiConfig;
use ort::environment::Environment;

use crate::embed::OnnxEmbedder;
use crate::ocr::PaddleOcr;
use crate::providers::device_is_usable;
use crate::runtime::{library_path, load_environment};

/// ONNX Runtime from the system, with the devices `/etc/crownos/ai.ron` set up.
#[derive(Debug)]
pub struct OrtBackend {
    environment: Result<Environment, InferError>,
    installed: Option<AiConfig>,
}

impl OrtBackend {
    /// Loads the ONNX Runtime library, not a model. A machine without it gets a backend
    /// whose probe finds no device.
    pub fn new() -> Self {
        let installed = AiConfig::load().unwrap_or_else(|error| {
            tracing::warn!(%error, "reading the AI setup; using every device");
            None
        });
        let environment = load_environment();
        if let Err(error) = &environment {
            tracing::info!(%error, "ONNX Runtime is not usable");
        }
        Self {
            environment,
            installed,
        }
    }

    fn environment(&self) -> Result<&Environment, InferError> {
        self.environment.as_ref().map_err(Clone::clone)
    }
}

impl Default for OrtBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeBackend for OrtBackend {
    fn runtime(&self) -> Runtime {
        Runtime::OnnxRuntime
    }

    fn version(&self) -> String {
        let library = library_path();
        let modified = std::fs::metadata(&library)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        let nvidia = std::fs::read_to_string("/sys/module/nvidia/version").unwrap_or_default();
        format!(
            "ort {} {}@{modified} nvidia {}",
            ort::MINOR_VERSION,
            library.display(),
            nvidia.trim()
        )
    }

    fn probe(&self, device: Device) -> bool {
        self.environment.is_ok()
            && device_is_usable(device, &library_path(), self.installed.as_ref())
    }

    fn load_embedder(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Embed>, InferError> {
        Ok(Arc::new(OnnxEmbedder::load(self.environment()?, spec)?))
    }

    fn load_ocr(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Ocr>, InferError> {
        Ok(Arc::new(PaddleOcr::load(self.environment()?, spec)?))
    }
}
