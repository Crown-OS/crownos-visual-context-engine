use crate::error::InferError;

pub trait Transcribe: Send + Sync {
    /// The text spoken in mono 16 kHz `samples`.
    fn transcribe(&self, samples: &[i16]) -> Result<String, InferError>;
}
