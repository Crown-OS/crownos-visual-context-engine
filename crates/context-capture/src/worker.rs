//! The analysis thread: one frame at a time, from the inbox to an outcome.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::analysis::{AnalysisConditions, CaptureOutcome, FrameAnalyzer};
use crate::capture_hold::CaptureHolds;
use crate::frame_inbox::FrameInbox;
use crate::metrics::CaptureMetrics;

/// How long a stop waits for the analysis thread.
const STOP_WAIT: Duration = Duration::from_secs(5);

const STOP_CHECK: Duration = Duration::from_millis(500);

/// Whether the machine runs on battery now.
pub type PowerProbe = Box<dyn Fn() -> bool + Send>;
/// Where outcomes go. It runs on the analysis thread.
pub type OutcomeSink = Box<dyn FnMut(CaptureOutcome) + Send>;

#[derive(Debug)]
pub(crate) struct AnalysisWorker {
    stop: Arc<AtomicBool>,
    inbox: Arc<FrameInbox>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) struct WorkerParts {
    pub(crate) inbox: Arc<FrameInbox>,
    pub(crate) analyzer: FrameAnalyzer,
    pub(crate) cancel: Arc<CaptureHolds>,
    pub(crate) metrics: Arc<CaptureMetrics>,
    pub(crate) on_battery: PowerProbe,
    pub(crate) deliver: OutcomeSink,
}

impl AnalysisWorker {
    pub(crate) fn spawn(parts: WorkerParts) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let inbox = Arc::clone(&parts.inbox);
        let thread = std::thread::Builder::new()
            .name("context-analysis".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                move || run(parts, &stop)
            })?;
        Ok(Self {
            stop,
            inbox,
            thread: Some(thread),
        })
    }
}

impl Drop for AnalysisWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && !context_core::join_within(thread, STOP_WAIT)
        {
            self.inbox.drain(Duration::ZERO);
            tracing::error!(waited = ?STOP_WAIT, "the analysis thread did not stop; its queued frames are zeroed, the frame it holds is not");
        }
    }
}

/// Marks the analysis as ended when dropped, also when the analysis panics.
struct AnalysisRunning<'a>(&'a FrameInbox);

impl Drop for AnalysisRunning<'_> {
    fn drop(&mut self) {
        self.0.analysis_done();
    }
}

fn run(mut parts: WorkerParts, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let Some(frame) = parts.inbox.take(STOP_CHECK) else {
            parts.inbox.expire();
            continue;
        };
        let running = AnalysisRunning(&parts.inbox);
        let conditions = AnalysisConditions {
            on_battery: (parts.on_battery)(),
            cancel: parts.cancel.as_ref(),
        };
        let outcome = parts.analyzer.analyse(frame, conditions);
        drop(running);
        CaptureMetrics::count(&parts.metrics.frames_analysed);
        if outcome.secret_found {
            CaptureMetrics::count(&parts.metrics.secrets_seen);
        }
        (parts.deliver)(outcome);
    }
}
