//! Where the capture session hands frames to the analysis thread.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::frame::CapturedFrame;
use crate::frame_queue::FrameQueue;

#[derive(Debug, Default)]
struct InboxState {
    queue: FrameQueue,
    analysing: bool,
}

#[derive(Debug, Default)]
pub(crate) struct FrameInbox {
    state: Mutex<InboxState>,
    changed: Condvar,
}

impl FrameInbox {
    pub(crate) fn push(&self, frame: CapturedFrame) {
        if let Some(mut state) = self.lock() {
            state.queue.push(frame, Instant::now());
            self.changed.notify_all();
        }
    }

    /// Waits up to `timeout` for a frame and marks the analysis as running.
    pub(crate) fn take(&self, timeout: Duration) -> Option<CapturedFrame> {
        let state = self.lock()?;
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| state.queue.is_empty())
            .ok()?;
        let frame = state.queue.pop(Instant::now())?;
        state.analysing = true;
        Some(frame)
    }

    /// Drops the frames that waited too long, without a new frame to wake the queue.
    pub(crate) fn expire(&self) {
        if let Some(mut state) = self.lock() {
            state.queue.expire(Instant::now());
        }
    }

    pub(crate) fn analysis_done(&self) {
        if let Some(mut state) = self.lock() {
            state.analysing = false;
            self.changed.notify_all();
        }
    }

    /// Drops every queued frame, then waits up to `timeout` for the running analysis to end.
    /// Returns whether no frame is left in the pipeline.
    pub(crate) fn drain(&self, timeout: Duration) -> bool {
        let Some(mut state) = self.lock() else {
            return false;
        };
        state.queue.clear();
        self.changed
            .wait_timeout_while(state, timeout, |state| state.analysing)
            .is_ok_and(|(state, _)| !state.analysing && state.queue.is_empty())
    }

    pub(crate) fn dropped_unanalysed(&self) -> u64 {
        self.lock()
            .map_or(0, |state| state.queue.dropped_unanalysed())
    }

    fn lock(&self) -> Option<MutexGuard<'_, InboxState>> {
        self.state.lock().ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_frames::frame_for_window;

    #[test]
    fn drain_drops_queued_frames_and_waits_for_the_analysis() {
        let inbox = FrameInbox::default();
        inbox.push(frame_for_window("a", 4, 4));
        inbox.push(frame_for_window("b", 4, 4));
        let taken = inbox.take(Duration::ZERO).unwrap();
        assert!(!inbox.drain(Duration::from_millis(10)));
        assert_eq!(inbox.dropped_unanalysed(), 1);
        drop(taken);
        inbox.analysis_done();
        assert!(inbox.drain(Duration::ZERO));
    }
}
