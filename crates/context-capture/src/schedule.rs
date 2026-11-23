//! When to capture. Each trigger kind is debounced: a new request of the same kind restarts
//! its settle delay. Due requests fold into one capture, and captures are at least two seconds
//! apart.

use std::collections::HashMap;

use context_core::{Micros, Timestamp, Trigger, WindowIdent};

pub(crate) const MIN_CAPTURE_INTERVAL: Micros = Micros::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingCapture {
    window: Option<WindowIdent>,
    at: Timestamp,
}

#[derive(Debug, Default)]
pub struct CaptureScheduler {
    pending: HashMap<Trigger, PendingCapture>,
    last_capture: Option<Timestamp>,
}

impl CaptureScheduler {
    pub fn request(&mut self, trigger: Trigger, window: Option<WindowIdent>, at: Timestamp) {
        self.pending.insert(trigger, PendingCapture { window, at });
    }

    /// When the next capture can be taken, if one is pending.
    pub fn next_due(&self) -> Option<Timestamp> {
        let earliest = self.pending.values().map(|pending| pending.at).min()?;
        Some(match self.last_capture {
            Some(last) => Timestamp(earliest.0.max(last.plus(MIN_CAPTURE_INTERVAL).0)),
            None => earliest,
        })
    }

    /// The capture to take at `now`: the latest of the due requests. The other due requests
    /// are folded into it.
    pub fn take_due(&mut self, now: Timestamp) -> Option<(Trigger, Option<WindowIdent>)> {
        if self
            .last_capture
            .is_some_and(|last| now.0 < last.plus(MIN_CAPTURE_INTERVAL).0)
        {
            return None;
        }
        let due: Vec<Trigger> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.at.0 <= now.0)
            .map(|(trigger, _)| *trigger)
            .collect();
        let latest = due
            .iter()
            .filter_map(|trigger| self.pending.get(trigger).map(|pending| (*trigger, pending)))
            .max_by_key(|(_, pending)| pending.at.0)
            .map(|(trigger, pending)| (trigger, pending.window.clone()))?;
        for trigger in due {
            self.pending.remove(&trigger);
        }
        self.last_capture = Some(now);
        Some(latest)
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn window() -> Option<WindowIdent> {
        Some(WindowIdent("w".into()))
    }

    #[test]
    fn a_repeated_trigger_restarts_its_delay() {
        let mut scheduler = CaptureScheduler::default();
        scheduler.request(Trigger::TypingPause, window(), Timestamp::from_secs(2));
        scheduler.request(Trigger::TypingPause, window(), Timestamp::from_secs(3));
        assert_eq!(scheduler.next_due(), Some(Timestamp::from_secs(3)));
        assert!(scheduler.take_due(Timestamp::from_secs(2)).is_none());
        assert_eq!(
            scheduler.take_due(Timestamp::from_secs(3)),
            Some((Trigger::TypingPause, window()))
        );
        assert_eq!(scheduler.next_due(), None);
    }

    #[test]
    fn due_requests_fold_into_one_capture_and_captures_stay_apart() {
        let mut scheduler = CaptureScheduler::default();
        scheduler.request(Trigger::FocusChange, window(), Timestamp::from_secs(1));
        scheduler.request(Trigger::Copy, window(), Timestamp::from_secs(2));
        assert_eq!(
            scheduler.take_due(Timestamp::from_secs(2)),
            Some((Trigger::Copy, window()))
        );
        scheduler.request(Trigger::ScrollStop, window(), Timestamp::from_secs(3));
        assert_eq!(scheduler.next_due(), Some(Timestamp::from_secs(4)));
        assert!(scheduler.take_due(Timestamp::from_secs(3)).is_none());
        assert!(scheduler.take_due(Timestamp::from_secs(4)).is_some());
    }
}
