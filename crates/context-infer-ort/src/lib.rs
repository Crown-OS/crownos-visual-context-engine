//! The ONNX Runtime backend: PP-OCR text recognition and text embeddings on the execution
//! providers that `/etc/crownos/ai.ron` set up. ONNX Runtime loads from the system at run
//! time, so one build serves every vendor.

mod backend;
mod embed;
mod ocr;
mod providers;
mod runtime;
mod session;
mod util;

pub use backend::OrtBackend;
pub use embed::OnnxEmbedder;
pub use ocr::PaddleOcr;
