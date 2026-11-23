use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Jobs of one cost class that may run at once; more get `Busy`. Interactive jobs have
/// places of their own on top, so background work never blocks the user.
#[derive(Debug, Clone)]
pub(crate) struct Admission {
    running: Arc<AtomicUsize>,
    limit: usize,
    interactive_extra: usize,
    retry_after: Duration,
}

/// Holds one place until the job ends.
#[derive(Debug)]
pub(crate) struct Permit {
    running: Arc<AtomicUsize>,
}

impl Admission {
    pub(crate) fn new(limit: usize, interactive_extra: usize, retry_after: Duration) -> Self {
        Self {
            running: Arc::new(AtomicUsize::new(0)),
            limit,
            interactive_extra,
            retry_after,
        }
    }

    /// A place, or how long to wait for one.
    pub(crate) fn admit(&self, interactive: bool) -> Result<Permit, Duration> {
        let limit = self.limit
            + if interactive {
                self.interactive_extra
            } else {
                0
            };
        self.running
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |running| {
                (running < limit).then_some(running + 1)
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
        let admission = Admission::new(1, 0, Duration::from_millis(100));
        let permit = admission.admit(false).unwrap();
        assert_eq!(
            admission.admit(false).unwrap_err(),
            Duration::from_millis(100)
        );
        drop(permit);
        assert!(admission.admit(false).is_ok());
    }

    #[test]
    fn background_work_leaves_a_place_for_the_user() {
        let admission = Admission::new(1, 1, Duration::from_millis(100));
        let _background = admission.admit(false).unwrap();
        assert!(admission.admit(false).is_err());
        let _interactive = admission.admit(true).unwrap();
        assert!(admission.admit(true).is_err());
    }
}
