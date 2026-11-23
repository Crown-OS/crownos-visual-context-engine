use std::ops::ControlFlow;

use crate::error::InferError;
use crate::request::{GenRequest, Generation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerateCapabilities {
    pub images: bool,
    pub json_schema: bool,
    pub context_tokens: u32,
}

/// Text generation, with images when the model has a vision projector.
pub trait Generate: Send + Sync {
    fn capabilities(&self) -> GenerateCapabilities;

    /// Gives each piece of text to `on_text` as it is made; `Break` stops the generation.
    /// Stops with [`InferError::Timeout`] at `request.job.deadline`.
    fn stream(
        &self,
        request: &GenRequest,
        on_text: &mut dyn FnMut(&str) -> ControlFlow<()>,
    ) -> Result<Generation, InferError>;

    fn generate(&self, request: &GenRequest) -> Result<Generation, InferError> {
        self.stream(request, &mut |_| ControlFlow::Continue(()))
    }
}
