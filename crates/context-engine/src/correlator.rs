//! Joins across the last few minutes: which window a copy came from, and when a capture is due.

use std::collections::VecDeque;

use context_core::{Micros, Observation, ObservationKind, SelectionKind, Timestamp, WindowIdent};

pub use context_core::Trigger;

const WINDOW: Micros = Micros::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Copy {
    t: Timestamp,
    window: Option<WindowIdent>,
}

/// A capture the correlator asks for, due at `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub trigger: Trigger,
    pub window: Option<WindowIdent>,
    pub at: Timestamp,
}

#[derive(Debug, Default)]
pub struct Correlator {
    copies: VecDeque<Copy>,
    focus: Option<WindowIdent>,
    last_title: Option<(WindowIdent, String)>,
}

impl Correlator {
    /// The captures `observation` calls for, and the bookkeeping behind them.
    pub fn observe(&mut self, observation: &Observation) -> Vec<Due> {
        self.expire(observation.t);
        let due = |trigger: Trigger, window: Option<WindowIdent>| Due {
            trigger,
            window,
            at: observation.t.plus(trigger.settle()),
        };
        match &observation.kind {
            ObservationKind::Focus(window) => {
                self.focus = window.clone();
                window
                    .as_ref()
                    .map(|window| vec![due(Trigger::FocusChange, Some(window.clone()))])
                    .unwrap_or_default()
            }
            ObservationKind::WindowChanged(record) => {
                let changed = self
                    .last_title
                    .as_ref()
                    .is_none_or(|(ident, title)| *ident != record.ident || *title != record.title);
                self.last_title = Some((record.ident.clone(), record.title.clone()));
                if changed && self.focus.as_ref() == Some(&record.ident) {
                    vec![due(Trigger::TitleChange, Some(record.ident.clone()))]
                } else {
                    Vec::new()
                }
            }
            ObservationKind::InputActivity { window, counts, .. } => {
                let mut dues = Vec::new();
                if counts.typing_paused && counts.total_keys() > 0 {
                    dues.push(due(Trigger::TypingPause, window.clone()));
                }
                if counts.scroll_stopped {
                    dues.push(due(Trigger::ScrollStop, window.clone()));
                }
                dues
            }
            ObservationKind::Selection {
                kind: SelectionKind::Clipboard,
                window,
                ..
            } => {
                self.copies.push_back(Copy {
                    t: observation.t,
                    window: window.clone(),
                });
                vec![due(
                    Trigger::Copy,
                    window.clone().or_else(|| self.focus.clone()),
                )]
            }
            _ => Vec::new(),
        }
    }

    /// Where the clipboard content a paste uses most likely came from.
    pub fn copy_source(&self, before: Timestamp) -> Option<&WindowIdent> {
        self.copies
            .iter()
            .rev()
            .find(|copy| copy.t.0 <= before.0)
            .and_then(|copy| copy.window.as_ref())
    }

    /// Copies seen in `range`, for a segment's importance.
    pub fn copies_between(&self, start: Timestamp, end: Timestamp) -> u32 {
        u32::try_from(
            self.copies
                .iter()
                .filter(|copy| copy.t.0 >= start.0 && copy.t.0 <= end.0)
                .count(),
        )
        .unwrap_or(u32::MAX)
    }

    fn expire(&mut self, now: Timestamp) {
        while self
            .copies
            .front()
            .is_some_and(|copy| now.since(copy.t).0 > WINDOW.0)
        {
            self.copies.pop_front();
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use context_core::InputCounts;

    use super::*;

    fn at(secs: i64, kind: ObservationKind) -> Observation {
        Observation::new(Timestamp::from_secs(secs), kind)
    }

    #[test]
    fn copies_remember_their_window_for_ten_minutes() {
        let mut correlator = Correlator::default();
        let w = WindowIdent("w".into());
        let dues = correlator.observe(&at(
            10,
            ObservationKind::Selection {
                kind: SelectionKind::Clipboard,
                window: Some(w.clone()),
                mime_types: vec![],
                size_hint: 0,
                secret: false,
            },
        ));
        assert_eq!(dues[0].trigger, Trigger::Copy);
        assert_eq!(dues[0].at, Timestamp::from_secs(10));
        assert_eq!(correlator.copy_source(Timestamp::from_secs(20)), Some(&w));
        correlator.observe(&at(700, ObservationKind::Focus(None)));
        assert_eq!(correlator.copy_source(Timestamp::from_secs(800)), None);
    }

    #[test]
    fn typing_pauses_and_focus_changes_schedule_captures() {
        let mut correlator = Correlator::default();
        let w = WindowIdent("w".into());
        let focus = correlator.observe(&at(0, ObservationKind::Focus(Some(w.clone()))));
        assert_eq!(focus[0].at, Timestamp(400_000));
        let paused = correlator.observe(&at(
            5,
            ObservationKind::InputActivity {
                window: Some(w),
                interval: Micros::from_secs(2),
                counts: InputCounts {
                    text_keys: 3,
                    typing_paused: true,
                    ..InputCounts::default()
                },
            },
        ));
        assert_eq!(paused[0].trigger, Trigger::TypingPause);
    }
}
