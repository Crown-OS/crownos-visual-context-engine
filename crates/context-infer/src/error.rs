use std::borrow::Cow;
use std::time::Duration;

/// Why an inference call gave no result.
#[derive(Clone, Debug, thiserror::Error)]
pub enum InferError {
    #[error("{0} is not available")]
    Unavailable(Cow<'static, str>),
    #[error("the deadline passed before a result")]
    Timeout,
    #[error("the job was cancelled")]
    Cancelled,
    #[error("the backend failed: {0}")]
    Backend(String),
    #[error("the output does not match the schema: {0}")]
    Schema(String),
    #[error("the backend is busy; retry after {retry_after:?}")]
    Busy { retry_after: Duration },
    /// The request itself is wrong (a NUL byte, a bad schema, a missing variable); no other
    /// candidate can do better.
    #[error("the request is not valid: {0}")]
    InvalidInput(String),
}
