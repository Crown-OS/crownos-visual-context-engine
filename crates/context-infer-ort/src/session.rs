use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;

use context_infer::{Device, InferError};
use ort::environment::Environment;
use ort::session::Session;

use crate::providers::execution_providers;
use crate::util::error::backend_error;

/// One ONNX graph on one device. `run` needs `&mut`, and a model thread makes one call at a
/// time, so the lock never waits.
#[derive(Debug)]
pub(crate) struct GraphSession {
    session: Mutex<Session>,
    input_names: Vec<String>,
    output_names: Vec<String>,
}

impl GraphSession {
    pub(crate) fn open(
        environment: &Environment,
        path: &Path,
        device: Device,
    ) -> Result<Self, InferError> {
        if !path.is_file() {
            return Err(InferError::Unavailable(
                format!("the model file {}", path.display()).into(),
            ));
        }
        let failed = |error: String| backend_error(&format!("opening {}", path.display()), error);
        let session = Session::builder(environment)
            .map_err(|error| failed(error.to_string()))?
            .with_intra_threads(background_threads())
            .map_err(|error| failed(error.to_string()))?
            .with_execution_providers(execution_providers(device))
            .map_err(|error| failed(error.to_string()))?
            .commit_from_file(path)
            .map_err(|error| failed(error.to_string()))?;
        let input_names = session
            .inputs()
            .iter()
            .map(|input| input.name().to_owned())
            .collect();
        let output_names = session
            .outputs()
            .iter()
            .map(|output| output.name().to_owned())
            .collect();
        Ok(Self {
            session: Mutex::new(session),
            input_names,
            output_names,
        })
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn input_names(&self) -> &[String] {
        &self.input_names
    }

    pub(crate) fn output_names(&self) -> &[String] {
        &self.output_names
    }

    pub(crate) fn first_input(&self) -> Result<&str, InferError> {
        self.input_names
            .first()
            .map(String::as_str)
            .ok_or_else(|| InferError::Backend("the graph has no input".into()))
    }

    pub(crate) fn first_output(&self) -> Result<&str, InferError> {
        self.output_names
            .first()
            .map(String::as_str)
            .ok_or_else(|| InferError::Backend("the graph has no output".into()))
    }
}

/// Half the cores: inference runs in the background of an interactive desktop.
fn background_threads() -> usize {
    thread::available_parallelism().map_or(1, |cores| (cores.get() / 2).max(1))
}
