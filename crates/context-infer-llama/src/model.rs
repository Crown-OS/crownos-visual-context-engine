use std::path::Path;
use std::thread;

use context_infer::{Device, InferError};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;

use crate::devices::device_indices;
use crate::util::error::backend_error;

/// Loads a GGUF file with every layer on `device`, or none for the CPU.
pub(crate) fn load_model(
    backend: &LlamaBackend,
    path: &Path,
    device: Device,
) -> Result<LlamaModel, InferError> {
    if !path.is_file() {
        return Err(InferError::Unavailable(
            format!("the model file {}", path.display()).into(),
        ));
    }
    let indices = device_indices(device)
        .ok_or_else(|| InferError::Unavailable(format!("the {device:?} device").into()))?;
    let gpu_layers = if device == Device::Cpu { 0 } else { u32::MAX };
    let params = LlamaModelParams::default()
        .with_n_gpu_layers(gpu_layers)
        .with_use_mmap(true)
        .with_devices(&indices)
        .map_err(|error| backend_error("choosing the device", error))?;
    LlamaModel::load_from_file(backend, path, &params)
        .map_err(|error| backend_error(&format!("loading {}", path.display()), error))
}

/// Half the cores: inference runs in the background of an interactive desktop.
pub(crate) fn background_threads() -> i32 {
    let cores = thread::available_parallelism().map_or(1, |cores| cores.get() / 2);
    i32::try_from(cores.max(1)).unwrap_or(1)
}
