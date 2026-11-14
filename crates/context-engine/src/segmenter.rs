//! Attention becomes segments: focus on one document for at least two seconds opens one,
//! heartbeats keep it open, a change of document or a long absence closes it.

use context_core::{
    DocKey, InputCounts, Micros, Segment, SegmentId, SegmentState, TimeRange, Timestamp,
    WindowIdent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmenterConfig {
    /// Focus shorter than this is a transition, not attention.
    pub open_after: Micros,
    /// Without a heartbeat for this long the segment is closed where the last one was.
    pub idle_close: Micros,
    /// Focus away for this long closes the segment where focus left.
    pub away_close: Micros,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            open_after: Micros::from_secs(2),
            idle_close: Micros::from_secs(120),
            away_close: Micros::from_secs(30),
        }
    }
}

/// What the segmenter needs to know about the focused window to open a segment on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focused {
    pub window: WindowIdent,
    pub app_key: String,
    pub doc_key: DocKey,
    pub title: String,
    pub url: Option<String>,
    pub file_path: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SegmentEvent {
    Opened(Segment),
    /// The open segment grew: more active time or input.
    Extended(Segment),
    Closed(Segment),
    /// Focus visited a window too briefly to count.
    Transition {
        to: WindowIdent,
        at: Timestamp,
    },
}

#[derive(Debug, Clone)]
struct Open {
    window: WindowIdent,
    segment: Segment,
    last_heartbeat: Timestamp,
    away_since: Option<Timestamp>,
}

#[derive(Debug, Clone)]
struct Candidate {
    focused: Focused,
    since: Timestamp,
}

#[derive(Debug)]
pub struct Segmenter {
    config: SegmenterConfig,
    next_id: u64,
    open: Option<Open>,
    candidate: Option<Candidate>,
}

impl Default for Segmenter {
    fn default() -> Self {
        Self::new(SegmenterConfig::default())
    }
}

impl Segmenter {
    pub const fn new(config: SegmenterConfig) -> Self {
        Self {
            config,
            next_id: 1,
            open: None,
            candidate: None,
        }
    }

    /// Continues ids after a restart.
    pub const fn with_next_id(mut self, next: u64) -> Self {
        self.next_id = next;
        self
    }

    pub fn open_segment(&self) -> Option<&Segment> {
        self.open.as_ref().map(|open| &open.segment)
    }

    pub fn open_window(&self) -> Option<&WindowIdent> {
        self.open.as_ref().map(|open| &open.window)
    }

    /// Focus moved to `focused`, or to nothing.
    pub fn focus(&mut self, focused: Option<Focused>, t: Timestamp) -> Vec<SegmentEvent> {
        let mut events = self.tick(t);
        let Some(focused) = focused else {
            self.leave(t);
            self.candidate = None;
            return events;
        };
        if let Some(open) = &mut self.open
            && open.window == focused.window
        {
            if open.away_since.take().is_some()
                && let Some(candidate) = self.candidate.take()
            {
                events.push(SegmentEvent::Transition {
                    to: candidate.focused.window,
                    at: candidate.since,
                });
            }
            open.last_heartbeat = t;
            return events;
        }
        self.leave(t);
        self.candidate = Some(Candidate { focused, since: t });
        events.extend(self.promote(t));
        events
    }

    /// Input in a window.
    pub fn heartbeat(
        &mut self,
        window: &WindowIdent,
        counts: &InputCounts,
        interval: Micros,
        t: Timestamp,
    ) -> Vec<SegmentEvent> {
        let mut events = self.tick(t);
        if let Some(open) = &mut self.open
            && open.window == *window
            && open.away_since.is_none()
        {
            open.last_heartbeat = t;
            open.segment.range.end = t;
            if !counts.is_empty() {
                open.segment.active = open.segment.active.saturating_add(interval);
                open.segment.input.add(counts);
            }
            events.push(SegmentEvent::Extended(open.segment.clone()));
        }
        events
    }

    /// The focused window's document changed: the segment ends and a new one begins at once.
    pub fn document_changed(&mut self, focused: Focused, t: Timestamp) -> Vec<SegmentEvent> {
        let mut events = self.tick(t);
        let on_open = self.open.as_ref().is_some_and(|open| {
            open.window == focused.window && open.segment.doc_key != focused.doc_key
        });
        if on_open {
            events.extend(self.close(t));
            self.candidate = Some(Candidate { focused, since: t });
            events.extend(self.promote_now(t));
        } else if let Some(candidate) = &mut self.candidate
            && candidate.focused.window == focused.window
        {
            candidate.focused = focused;
        }
        events
    }

    /// The session locked or went idle: nothing is being attended to.
    pub fn interrupt(&mut self, t: Timestamp) -> Vec<SegmentEvent> {
        let mut events = self.tick(t);
        events.extend(self.close(t));
        self.candidate = None;
        events
    }

    pub fn window_closed(&mut self, window: &WindowIdent, t: Timestamp) -> Vec<SegmentEvent> {
        let mut events = self.tick(t);
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.window == *window)
        {
            events.extend(self.close(t));
        }
        if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.focused.window == *window)
        {
            self.candidate = None;
        }
        events
    }

    /// Time passed: closes what went stale and opens what has been focused long enough.
    pub fn tick(&mut self, t: Timestamp) -> Vec<SegmentEvent> {
        let mut events = Vec::new();
        if let Some(open) = &self.open {
            if let Some(away) = open.away_since
                && t.since(away).0 >= self.config.away_close.0
            {
                events.extend(self.close(away));
            } else if open.away_since.is_none()
                && t.since(open.last_heartbeat).0 >= self.config.idle_close.0
            {
                let end = open.last_heartbeat;
                events.extend(self.close(end));
            }
        }
        events.extend(self.promote(t));
        events
    }

    const fn leave(&mut self, t: Timestamp) {
        if let Some(open) = &mut self.open
            && open.away_since.is_none()
        {
            open.away_since = Some(t);
        }
    }

    fn promote(&mut self, t: Timestamp) -> Vec<SegmentEvent> {
        let ready = self
            .candidate
            .as_ref()
            .is_some_and(|candidate| t.since(candidate.since).0 >= self.config.open_after.0);
        if !ready {
            return Vec::new();
        }
        self.promote_now(t)
    }

    /// Attention already shown continues on the new document, so no waiting period applies.
    fn promote_now(&mut self, t: Timestamp) -> Vec<SegmentEvent> {
        let Some(candidate) = self.candidate.take() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        if let Some(open) = &self.open {
            let end = open.away_since.unwrap_or(candidate.since);
            events.extend(self.close(end));
        }
        let segment = Segment {
            id: SegmentId(self.next_id),
            episode: None,
            app_key: candidate.focused.app_key,
            doc_key: candidate.focused.doc_key,
            title: candidate.focused.title,
            url: candidate.focused.url,
            file_path: candidate.focused.file_path,
            cwd: candidate.focused.cwd,
            project: None,
            range: TimeRange::new(candidate.since, t),
            active: Micros::ZERO,
            input: Default::default(),
            label: Default::default(),
            label_confidence: 0.0,
            importance: 0.0,
            text_coverage: 0.0,
            summary: None,
            state: SegmentState::Open,
        };
        self.next_id += 1;
        self.open = Some(Open {
            window: candidate.focused.window,
            segment: segment.clone(),
            last_heartbeat: t,
            away_since: None,
        });
        events.push(SegmentEvent::Opened(segment));
        events
    }

    fn close(&mut self, end: Timestamp) -> Vec<SegmentEvent> {
        let Some(mut open) = self.open.take() else {
            return Vec::new();
        };
        if end.0 >= open.segment.range.start.0 {
            open.segment.range.end = end;
        }
        open.segment.state = SegmentState::Closed;
        vec![SegmentEvent::Closed(open.segment)]
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
    use super::*;

    fn focused(window: &str, doc: &str) -> Focused {
        Focused {
            window: WindowIdent(window.into()),
            app_key: "app".into(),
            doc_key: DocKey::Title(doc.into()),
            title: doc.into(),
            url: None,
            file_path: None,
            cwd: None,
        }
    }

    fn secs(s: i64) -> Timestamp {
        Timestamp::from_secs(s)
    }

    fn typing() -> InputCounts {
        InputCounts {
            text_keys: 10,
            ..InputCounts::default()
        }
    }

    #[test]
    fn two_seconds_of_focus_open_a_segment_and_a_glance_does_not() {
        let mut segmenter = Segmenter::default();
        assert!(
            segmenter
                .focus(Some(focused("a", "one")), secs(0))
                .is_empty()
        );
        let opened = segmenter.heartbeat(
            &WindowIdent("a".into()),
            &typing(),
            Micros::from_secs(2),
            secs(2),
        );
        assert!(matches!(opened[0], SegmentEvent::Opened(ref s) if s.range.start == secs(0)));
        segmenter.focus(Some(focused("b", "two")), secs(10));
        let back = segmenter.focus(Some(focused("a", "one")), secs(11));
        assert!(
            matches!(back[0], SegmentEvent::Transition { ref to, at } if to.0 == "b" && at == secs(10))
        );
        assert!(segmenter.open_segment().is_some());
    }

    #[test]
    fn moving_on_closes_where_focus_left() {
        let mut segmenter = Segmenter::default();
        segmenter.focus(Some(focused("a", "one")), secs(0));
        segmenter.heartbeat(
            &WindowIdent("a".into()),
            &typing(),
            Micros::from_secs(10),
            secs(10),
        );
        segmenter.focus(Some(focused("b", "two")), secs(20));
        let events = segmenter.heartbeat(
            &WindowIdent("b".into()),
            &typing(),
            Micros::from_secs(10),
            secs(30),
        );
        let closed = events.iter().find_map(|e| match e {
            SegmentEvent::Closed(s) => Some(s.clone()),
            _ => None,
        });
        let closed = closed.unwrap();
        assert_eq!(closed.range, TimeRange::new(secs(0), secs(20)));
        assert_eq!(closed.active, Micros::from_secs(10));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SegmentEvent::Opened(s) if s.range.start == secs(20)))
        );
    }

    #[test]
    fn a_document_change_splits_in_place_and_idleness_closes() {
        let mut segmenter = Segmenter::default();
        segmenter.focus(Some(focused("a", "one")), secs(0));
        segmenter.heartbeat(
            &WindowIdent("a".into()),
            &typing(),
            Micros::from_secs(5),
            secs(5),
        );
        let split = segmenter.document_changed(focused("a", "two"), secs(50));
        assert!(matches!(split[0], SegmentEvent::Closed(ref s) if s.range.end == secs(50)));
        assert!(matches!(split[1], SegmentEvent::Opened(ref s) if s.range.start == secs(50)));
        let idle = segmenter.tick(secs(50 + 121));
        assert!(matches!(idle[0], SegmentEvent::Closed(ref s) if s.range.end == secs(50)));
        assert!(segmenter.open_segment().is_none());
    }
}
