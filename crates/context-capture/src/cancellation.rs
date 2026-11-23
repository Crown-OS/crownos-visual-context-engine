use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Whether running work should stop: the system is going to sleep, observation is paused, or
/// the process is about to be readable.
pub trait Cancellation: Sync + std::fmt::Debug {
    fn is_cancelled(&self) -> bool;
}

impl Cancellation for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Relaxed)
    }
}

/// When a model call must end, and what can stop it sooner.
#[derive(Debug, Clone, Copy)]
pub struct ModelCall<'a> {
    pub deadline: Instant,
    pub cancel: &'a dyn Cancellation,
}

const CANCEL_CHECK: Duration = Duration::from_millis(20);

impl ModelCall<'_> {
    /// Sleeps for `duration`. Returns `false` at once when the call is cancelled.
    pub fn sleep(&self, duration: Duration) -> bool {
        let end = Instant::now() + duration;
        loop {
            if self.cancel.is_cancelled() {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(CANCEL_CHECK));
        }
    }

    /// Runs `work` on its own thread and waits for it. Returns `None` at once when the call is
    /// cancelled; the work then ends on its own, and what it holds goes with it.
    pub fn wait_for<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (answer, answered) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("context-model-call".to_owned())
            .spawn(move || {
                let _ = answer.send(work());
            })
            .ok()?;
        loop {
            if self.cancel.is_cancelled() {
                return None;
            }
            match answered.recv_timeout(CANCEL_CHECK) {
                Ok(value) => return Some(value),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_ends_a_sleep_and_a_wait_at_once() {
        let cancel = AtomicBool::new(true);
        let call = ModelCall {
            deadline: Instant::now() + Duration::from_secs(10),
            cancel: &cancel,
        };
        let started = Instant::now();
        assert!(!call.sleep(Duration::from_secs(5)));
        assert!(
            call.wait_for(|| std::thread::sleep(Duration::from_secs(1)))
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(call.wait_for(|| 7), Some(7));
    }
}
