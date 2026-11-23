use std::borrow::Cow;
use std::time::Duration;

/// Why an inference call gave no result.
#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("{0} is not available")]
    Unavailable(Cow<'static, str>),
    #[error("the deadline passed before a result")]
    Timeout,
    #[error("the backend failed: {0}")]
    Backend(String),
    #[error("the output does not match the schema: {0}")]
    Schema(String),
    #[error("the backend is busy; retry after {retry_after:?}")]
    Busy { retry_after: Duration },
}

impl InferError {
    /// A failure of the runtime or device, not of the request: the router opens its breaker.
    /// A backend that stops at the deadline did its job, so its `Timeout` does not count.
    pub(crate) const fn trips_breaker(&self) -> bool {
        matches!(self, Self::Backend(_))
    }
}
