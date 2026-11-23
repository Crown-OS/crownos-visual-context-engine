//! The capture pipeline as one value: the session that captures, the RAM-only inbox, and the
//! analysis thread.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use context_collect::BridgeLink;

use crate::analysis::FrameAnalyzer;
use crate::capture_hold::{CaptureHold, CaptureHolds};
use crate::frame::CaptureTarget;
use crate::frame_inbox::FrameInbox;
use crate::metrics::CaptureMetrics;
use crate::screen_models::ScreenModels;
use crate::session::{CaptureSession, SessionError};
use crate::structured_text::StructuredText;
use crate::worker::{AnalysisWorker, OutcomeSink, PowerProbe, WorkerParts};

/// What the pipeline needs from the daemon.
pub struct CaptureServiceParts {
    pub models: Arc<dyn ScreenModels>,
    pub structured_text: Box<dyn StructuredText>,
    pub on_battery: PowerProbe,
    pub deliver: OutcomeSink,
}

impl std::fmt::Debug for CaptureServiceParts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CaptureServiceParts")
            .finish_non_exhaustive()
    }
}

/// Counts for the status API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureCounts {
    pub frames_captured: u64,
    pub frames_analysed: u64,
    pub frame_dropped_unanalysed: u64,
    pub captures_denied: u64,
    pub captures_unchanged: u64,
    pub captures_failed: u64,
    pub secrets_seen: u64,
}

#[derive(Debug)]
pub struct CaptureService {
    session: CaptureSession,
    _worker: AnalysisWorker,
    inbox: Arc<FrameInbox>,
    metrics: Arc<CaptureMetrics>,
    holds: Arc<CaptureHolds>,
}

impl CaptureService {
    /// # Errors
    ///
    /// Fails when a thread cannot be started.
    pub fn spawn(link: BridgeLink, parts: CaptureServiceParts) -> Result<Self, SessionError> {
        let inbox = Arc::new(FrameInbox::default());
        let metrics = Arc::new(CaptureMetrics::default());
        let holds = Arc::new(CaptureHolds::default());
        let worker = AnalysisWorker::spawn(WorkerParts {
            inbox: Arc::clone(&inbox),
            analyzer: FrameAnalyzer::new(parts.models, parts.structured_text),
            cancel: Arc::clone(&holds),
            metrics: Arc::clone(&metrics),
            on_battery: parts.on_battery,
            deliver: parts.deliver,
        })?;
        let session = CaptureSession::spawn(
            link,
            Arc::clone(&inbox),
            Arc::clone(&metrics),
            Arc::clone(&holds),
        )?;
        Ok(Self {
            session,
            _worker: worker,
            inbox,
            metrics,
            holds,
        })
    }

    pub fn capture(&self, target: CaptureTarget) {
        if !self.holds.is_held() {
            self.session.capture(target);
        }
    }

    pub fn end_session(&self) {
        self.session.end_session();
    }

    /// Stops new captures, aborts the one in flight, drops the queued frames, and waits up to
    /// `timeout` in all for the running analysis to end. Every frame is zeroed when this returns
    /// `true`. Captures and model calls stay off until every hold is released.
    pub fn suspend(&self, reason: CaptureHold, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        self.holds.hold(reason);
        let aborted = self.session.abort(timeout);
        let drained = self
            .inbox
            .drain(deadline.saturating_duration_since(Instant::now()));
        aborted && drained
    }

    /// Releases one hold. Captures and analysis run again when none is left.
    pub fn resume(&self, reason: CaptureHold) {
        self.holds.release(reason);
    }

    pub fn counts(&self) -> CaptureCounts {
        let read = |counter: &std::sync::atomic::AtomicU64| counter.load(Ordering::Relaxed);
        CaptureCounts {
            frames_captured: read(&self.metrics.frames_captured),
            frames_analysed: read(&self.metrics.frames_analysed),
            frame_dropped_unanalysed: self.inbox.dropped_unanalysed(),
            captures_denied: read(&self.metrics.captures_denied),
            captures_unchanged: read(&self.metrics.captures_unchanged),
            captures_failed: read(&self.metrics.captures_failed),
            secrets_seen: read(&self.metrics.secrets_seen),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::cancellation::ModelCall;
    use crate::geometry::Rect;
    use crate::region_copy::RegionCopy;
    use crate::screen_models::{ModelError, RecognizedLine, ScreenModels, ScreenSummary};
    use crate::structured_text::NoStructuredText;
    use crate::test_frames::{frame_for, is_all_zero, memfd_reader, paint_rect, target};

    struct SlowModels;

    impl ScreenModels for SlowModels {
        fn recognize_text(
            &self,
            _: &RegionCopy,
            _: &ModelCall<'_>,
        ) -> Result<Vec<RecognizedLine>, ModelError> {
            std::thread::sleep(Duration::from_millis(300));
            Ok(Vec::new())
        }

        fn describe_screen(
            &self,
            _: &RegionCopy,
            _: &str,
            _: &str,
            _: &ModelCall<'_>,
        ) -> Result<ScreenSummary, ModelError> {
            Err(ModelError::Unavailable("vision".to_owned()))
        }
    }

    #[test]
    fn suspend_zeroes_the_queued_frame_and_waits_for_the_running_one() {
        let service = CaptureService::spawn(
            BridgeLink::default(),
            CaptureServiceParts {
                models: Arc::new(SlowModels),
                structured_text: Box::new(NoStructuredText),
                on_battery: Box::new(|| true),
                deliver: Box::new(|_| {}),
            },
        )
        .unwrap();
        let painted = |window: &str| {
            frame_for(
                target(window),
                64,
                32,
                |pixels| paint_rect(pixels, 64, Rect::new(4, 4, 30, 8)),
                vec![],
            )
        };
        let running = painted("a");
        let queued = painted("b");
        let running_reader = memfd_reader(&running);
        let queued_reader = memfd_reader(&queued);
        service.inbox.push(running);
        std::thread::sleep(Duration::from_millis(100));
        service.inbox.push(queued);

        assert!(service.suspend(CaptureHold::Sleep, Duration::from_secs(2)));
        assert!(is_all_zero(&queued_reader));
        assert!(is_all_zero(&running_reader));
        assert_eq!(service.counts().frame_dropped_unanalysed, 1);
        service.resume(CaptureHold::Sleep);
    }
}
