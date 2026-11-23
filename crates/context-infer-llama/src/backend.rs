use std::path::Path;
use std::sync::Arc;

use context_infer::{Device, Embed, Generate, InferError, LoadSpec, Runtime, RuntimeBackend};
use crownos_config::system::ai::AiConfig;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::{LogOptions, list_llama_ggml_backend_devices, send_logs_to_tracing};

use crate::devices::device_indices;
use crate::embedder::GgufEmbedder;
use crate::generator::GgufGenerator;
use crate::util::error::backend_error;

/// Where the package installs the ggml device modules (`libggml-vulkan.so`, ...).
pub const DEFAULT_GGML_MODULES_DIR: &str = "/usr/lib/crown-context/ggml";

/// llama.cpp for the whole process, with the ggml device modules found in one directory.
#[derive(Debug)]
pub struct LlamaBackendHost {
    backend: Arc<LlamaBackend>,
    installed: Option<AiConfig>,
}

impl LlamaBackendHost {
    /// Loads the ggml modules from `modules_dir`, or from the directory of the build when it
    /// is `None`. Only one host can exist in a process.
    pub fn new(modules_dir: Option<&Path>) -> Result<Self, InferError> {
        match modules_dir {
            Some(directory) if directory.to_str().is_some() => {
                llama_cpp_2::llama_backend::load_backends_from_path(directory);
            }
            Some(directory) => {
                return Err(InferError::Unavailable(
                    format!("ggml modules in the non-UTF-8 path {}", directory.display()).into(),
                ));
            }
            None => llama_cpp_2::llama_backend::load_backends(),
        }
        send_logs_to_tracing(LogOptions::default());
        let backend =
            LlamaBackend::init().map_err(|error| backend_error("starting llama.cpp", error))?;
        let installed = AiConfig::load().unwrap_or_else(|error| {
            tracing::warn!(%error, "reading the AI setup; using every device");
            None
        });
        Ok(Self {
            backend: Arc::new(backend),
            installed,
        })
    }
}

impl RuntimeBackend for LlamaBackendHost {
    fn runtime(&self) -> Runtime {
        Runtime::LlamaCpp
    }

    fn version(&self) -> String {
        let devices: Vec<String> = list_llama_ggml_backend_devices()
            .into_iter()
            .map(|device| format!("{}={}", device.name, device.description))
            .collect();
        let nvidia = std::fs::read_to_string("/sys/module/nvidia/version").unwrap_or_default();
        format!(
            "llama-cpp-2 {} [{}] nvidia {}",
            env!("CARGO_PKG_VERSION"),
            devices.join(","),
            nvidia.trim()
        )
    }

    fn probe(&self, device: Device) -> bool {
        let set_up = device == Device::Cpu
            || self
                .installed
                .as_ref()
                .is_none_or(|config| config.device_rank(device).is_some());
        Runtime::LlamaCpp.supports(device) && set_up && device_indices(device).is_some()
    }

    fn load_generator(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Generate>, InferError> {
        Ok(Arc::new(GgufGenerator::load(
            Arc::clone(&self.backend),
            spec,
        )?))
    }

    fn load_embedder(&self, spec: &LoadSpec<'_>) -> Result<Arc<dyn Embed>, InferError> {
        Ok(Arc::new(GgufEmbedder::load(
            Arc::clone(&self.backend),
            spec,
        )?))
    }
}
