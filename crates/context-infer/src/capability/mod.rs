//! What a loaded model can do. A backend crate implements these traits for each runtime.

mod embed;
mod generate;
mod ocr;
mod rerank;
mod transcribe;

pub use embed::{Embed, EmbedPurpose, EmbedSpace, Embeddings};
pub use generate::{Generate, GenerateCapabilities};
pub use ocr::{Ocr, TextBounds, TextLine};
pub use rerank::Rerank;
pub use transcribe::Transcribe;
