//! The llama.cpp backend: text and image generation with JSON-schema grammars, and GGUF text
//! embeddings. The ggml device backends (CUDA, Vulkan, ROCm, CPU variants) load as modules
//! from a directory at run time, so one build serves every vendor.

mod backend;
mod devices;
mod embedder;
mod generator;
mod grammar;
mod model;
mod util;

pub use backend::{DEFAULT_GGML_MODULES_DIR, LlamaBackendHost};
pub use embedder::GgufEmbedder;
pub use generator::GgufGenerator;
