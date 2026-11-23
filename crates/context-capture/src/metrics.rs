use std::sync::atomic::{AtomicU64, Ordering};

/// Counters of the capture pipeline. Each is a count of captures.
#[derive(Debug, Default)]
pub(crate) struct CaptureMetrics {
    pub frames_captured: AtomicU64,
    pub frames_analysed: AtomicU64,
    /// The compositor refused the frame: the window is sensitive or gone.
    pub captures_denied: AtomicU64,
    /// Nothing changed in the window before the wait ended.
    pub captures_unchanged: AtomicU64,
    pub captures_failed: AtomicU64,
    pub secrets_seen: AtomicU64,
}

impl CaptureMetrics {
    pub(crate) fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}
