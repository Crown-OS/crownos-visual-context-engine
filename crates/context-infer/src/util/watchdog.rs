use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Instant;

use crate::error::InferError;

/// Runs `work` on its own thread and stops the wait at `deadline`. A late thread runs on
/// detached; its result is dropped.
pub(crate) fn run_until<R: Send + 'static>(
    deadline: Instant,
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, InferError> {
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("context-infer-call".into())
        .spawn(move || {
            let _ = result_sender.send(work());
        })
        .map_err(|error| InferError::Backend(format!("could not start a call thread: {error}")))?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    result_receiver
        .recv_timeout(remaining)
        .map_err(|error| match error {
            RecvTimeoutError::Timeout => InferError::Timeout,
            RecvTimeoutError::Disconnected => {
                InferError::Backend("the call thread stopped without a result".into())
            }
        })
}
