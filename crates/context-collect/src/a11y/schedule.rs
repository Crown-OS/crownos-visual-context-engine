use std::time::Duration;

use context_core::Trigger;
use tokio::time::Instant;

/// Typing keeps pushing a read back, but never further than this from the first keystroke.
const MOST_TYPING_DELAY: Duration = Duration::from_secs(5);

/// Why the focused application should be read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wake {
    FocusChange,
    TitleChange,
    DocumentLoaded,
    Typing,
}

impl Wake {
    fn settle(self) -> Duration {
        let trigger = match self {
            Self::FocusChange => Trigger::FocusChange,
            Self::TitleChange | Self::DocumentLoaded => Trigger::TitleChange,
            Self::Typing => Trigger::TypingPause,
        };
        Duration::from_micros(u64::try_from(trigger.settle().0).unwrap_or(0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRead {
    application: String,
    first_wake: Instant,
    due: Instant,
}

/// The one read that is waiting: of the focused application, once things settle.
#[derive(Debug, Default)]
pub(crate) struct ReadSchedule {
    pending: Option<PendingRead>,
}

impl ReadSchedule {
    pub(crate) fn wake(&mut self, application: &str, wake: Wake, now: Instant) {
        let settled = now + wake.settle();
        match &mut self.pending {
            Some(pending) if pending.application == application => {
                pending.due = match wake {
                    Wake::Typing => settled.min(pending.first_wake + MOST_TYPING_DELAY),
                    _ => settled.min(pending.due),
                };
            }
            _ => {
                self.pending = Some(PendingRead {
                    application: application.to_owned(),
                    first_wake: now,
                    due: settled,
                });
            }
        }
    }

    pub(crate) fn due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.due)
    }

    /// The application to read now, if its time has come.
    pub(crate) fn take_due(&mut self, now: Instant) -> Option<String> {
        if self.due().is_some_and(|due| due <= now) {
            return self.pending.take().map(|pending| pending.application);
        }
        None
    }

    /// Puts a read back until its application may be read again.
    pub(crate) fn defer(&mut self, application: String, until: Instant) {
        self.pending = Some(PendingRead {
            application,
            first_wake: until,
            due: until,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_settles_before_the_read() {
        let now = Instant::now();
        let mut schedule = ReadSchedule::default();
        schedule.wake(":1.4", Wake::FocusChange, now);
        assert_eq!(schedule.take_due(now), None);
        let due = schedule.due().unwrap_or(now);
        assert_eq!(due, now + Duration::from_millis(400));
        assert_eq!(schedule.take_due(due).as_deref(), Some(":1.4"));
        assert_eq!(schedule.due(), None);
    }

    #[test]
    fn typing_waits_for_a_pause_but_not_forever() {
        let start = Instant::now();
        let mut schedule = ReadSchedule::default();
        for keystroke in 0..60 {
            schedule.wake(
                ":1.4",
                Wake::Typing,
                start + Duration::from_millis(keystroke * 100),
            );
        }
        assert_eq!(schedule.due(), Some(start + MOST_TYPING_DELAY));
    }

    #[test]
    fn a_focus_change_to_another_application_replaces_the_read() {
        let now = Instant::now();
        let mut schedule = ReadSchedule::default();
        schedule.wake(":1.4", Wake::Typing, now);
        schedule.wake(":1.9", Wake::FocusChange, now);
        let due = schedule.due().unwrap_or(now);
        assert_eq!(schedule.take_due(due).as_deref(), Some(":1.9"));
    }
}
