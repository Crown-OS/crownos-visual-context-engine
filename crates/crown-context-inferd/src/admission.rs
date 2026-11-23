use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Jobs of one cost class that may run at once; more get `Busy`.
#[derive(Debug, Clone)]
pub(crate) struct Admission {
    running: Arc<AtomicUsize>,
    limit: usize,
    retry_after: Duration,
}

/// Holds one place until the job ends.
#[derive(Debug)]
pub(crate) struct Permit {
    running: Arc<AtomicUsize>,
}

impl Admission {
    pub(crate) fn new(limit: usize, retry_after: Duration) -> Self {
        Self {
            running: Arc::new(AtomicUsize::new(0)),
            limit,
            retry_after,
        }
    }

    /// A place, or how long to wait for one.
    pub(crate) fn admit(&self) -> Result<Permit, Duration> {
        self.running
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |running| {
                (running < self.limit).then_some(running + 1)
            })
            .map(|_| Permit {
                running: Arc::clone(&self.running),
            })
            .map_err(|_| self.retry_after)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn refuses_above_the_limit_until_a_permit_ends() {
        let admission = Admission::new(1, Duration::from_millis(100));
        let permit = admission.admit().unwrap();
        assert_eq!(admission.admit().unwrap_err(), Duration::from_millis(100));
        drop(permit);
        assert!(admission.admit().is_ok());
    }
}
