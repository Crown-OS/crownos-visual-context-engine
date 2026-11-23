use std::fmt::Display;

use context_infer::InferError;

pub(crate) fn backend_error(context: &str, error: impl Display) -> InferError {
    InferError::Backend(format!("{context}: {error}"))
}
