use std::path::PathBuf;

use context_infer::InferError;
use ort::environment::{Environment, GlobalThreadPoolOptions};

const LIBRARY_VARIABLE: &str = "ORT_DYLIB_PATH";
/// Installed by the `onnxruntime-crownos` package.
const INSTALLED_LIBRARY: &str = "/usr/lib/libonnxruntime.so";
const ENVIRONMENT_NAME: &str = "crown-context-inferd";

/// `$ORT_DYLIB_PATH` if set, else the packaged `libonnxruntime.so`.
pub(crate) fn library_path() -> PathBuf {
    std::env::var_os(LIBRARY_VARIABLE)
        .filter(|path| !path.is_empty())
        .map_or_else(|| PathBuf::from(INSTALLED_LIBRARY), PathBuf::from)
}

/// Loads ONNX Runtime into the process with one thread pool for every session.
pub(crate) fn load_environment() -> Result<Environment, InferError> {
    let path = library_path();
    if !path.is_file() {
        return Err(InferError::Unavailable(
            format!("ONNX Runtime at {}", path.display()).into(),
        ));
    }
    let unavailable = |error: String| {
        InferError::Unavailable(format!("ONNX Runtime at {} ({error})", path.display()).into())
    };
    ort::init_from(&path)
        .map_err(|error| unavailable(error.to_string()))?
        .with_name(ENVIRONMENT_NAME)
        .with_global_thread_pool(GlobalThreadPoolOptions::default())
        .build()
        .map_err(|error| unavailable(error.to_string()))
}
