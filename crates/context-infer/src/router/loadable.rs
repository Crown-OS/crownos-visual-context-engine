use std::sync::Arc;

use super::backend::{LoadSpec, RuntimeBackend};
use crate::capability::{Embed, Generate, Ocr, Rerank, Transcribe};
use crate::error::InferError;
use crate::registry::Capability;

/// A capability trait object that the router can load through a [`RuntimeBackend`].
pub(crate) trait Loadable: Send + Sync + 'static {
    const CAPABILITY: Capability;

    fn load(backend: &dyn RuntimeBackend, spec: &LoadSpec<'_>) -> Result<Arc<Self>, InferError>;
}

macro_rules! loadable {
    ($capability_trait:ident, $capability:ident, $load:ident) => {
        impl Loadable for dyn $capability_trait {
            const CAPABILITY: Capability = Capability::$capability;

            fn load(
                backend: &dyn RuntimeBackend,
                spec: &LoadSpec<'_>,
            ) -> Result<Arc<Self>, InferError> {
                backend.$load(spec)
            }
        }
    };
}

loadable!(Generate, Generate, load_generator);
loadable!(Embed, Embed, load_embedder);
loadable!(Rerank, Rerank, load_reranker);
loadable!(Ocr, Ocr, load_ocr);
loadable!(Transcribe, Transcribe, load_transcriber);
