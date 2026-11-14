//! The engine proper: windows and apps on one side, segments, episodes and working memory on the
//! other, driven one observation at a time.

use std::collections::BTreeMap;

use context_core::{
    AppRef, Observation, ObservationKind, Segment, SessionState, Timestamp, WindowIdent,
    WindowRecord,
};
use context_protocol::CurrentContext;

use crate::classifier::classify;
use crate::correlator::{Correlator, Due};
use crate::doc_key::derive_doc_key;
use crate::episodes::{EpisodeBuilder, EpisodeEvent};
use crate::importance::importance;
use crate::segmenter::{Focused, SegmentEvent, Segmenter};
use crate::working_memory::WorkingMemory;

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    Segment(SegmentEvent),
    Episode(EpisodeEvent),
    /// A capture is wanted.
    Capture(Due),
    ContextChanged(CurrentContext),
}

#[derive(Debug)]
pub struct Engine {
    windows: BTreeMap<WindowIdent, WindowRecord>,
    apps: BTreeMap<WindowIdent, AppRef>,
    urls: BTreeMap<WindowIdent, String>,
    segmenter: Segmenter,
    episodes: EpisodeBuilder,
    correlator: Correlator,
    working: WorkingMemory,
    home: Option<String>,
    observing: bool,
    session: SessionState,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(std::env::var("HOME").ok())
    }
}

impl Engine {
    pub fn new(home: Option<String>) -> Self {
        Self {
            windows: BTreeMap::new(),
            apps: BTreeMap::new(),
            urls: BTreeMap::new(),
            segmenter: Segmenter::default(),
            episodes: EpisodeBuilder::default(),
            correlator: Correlator::default(),
            working: WorkingMemory::default(),
            home,
            observing: true,
            session: SessionState::Active,
        }
    }

    /// Continues segment and episode ids after a restart.
    pub fn with_next_ids(mut self, segment: u64, episode: u64) -> Self {
        self.segmenter = Segmenter::default().with_next_id(segment);
        self.episodes = EpisodeBuilder::default().with_next_id(episode);
        self
    }

    pub fn restore_working_memory(&mut self, memory: WorkingMemory) {
        self.working = memory;
    }

    pub const fn working_memory(&self) -> &WorkingMemory {
        &self.working
    }

    pub fn window(&self, ident: &WindowIdent) -> Option<&WindowRecord> {
        self.windows.get(ident)
    }

    pub const fn is_observing(&self) -> bool {
        self.observing && !matches!(self.session, SessionState::Locked)
    }

    /// What the daemon resolved the window's app to; before this, the app id stands in.
    pub fn set_app(&mut self, window: &WindowIdent, app: AppRef) {
        self.apps.insert(window.clone(), app);
    }

    pub fn current_context(&self, now: Timestamp) -> CurrentContext {
        self.working.current_context(now, self.is_observing())
    }

    pub fn ingest(&mut self, observation: &Observation) -> Vec<EngineEvent> {
        let t = observation.t;
        let mut events: Vec<EngineEvent> = self
            .correlator
            .observe(observation)
            .into_iter()
            .map(EngineEvent::Capture)
            .collect();
        let segment_events = match &observation.kind {
            ObservationKind::WindowOpened(record) => {
                self.windows.insert(record.ident.clone(), record.clone());
                self.working.open_documents = u32::try_from(self.windows.len()).unwrap_or(u32::MAX);
                self.segmenter.tick(t)
            }
            ObservationKind::WindowChanged(record) => {
                self.windows.insert(record.ident.clone(), record.clone());
                let focused = self.focused(&record.ident);
                let events = match focused {
                    Some(focused) => self.segmenter.document_changed(focused, t),
                    None => self.segmenter.tick(t),
                };
                if self.working.focus.as_ref() == Some(&record.ident) {
                    self.working.focus_title = record.title.clone();
                }
                events
            }
            ObservationKind::WindowClosed(ident) => {
                self.windows.remove(ident);
                self.apps.remove(ident);
                self.urls.remove(ident);
                self.working.open_documents = u32::try_from(self.windows.len()).unwrap_or(u32::MAX);
                self.segmenter.window_closed(ident, t)
            }
            ObservationKind::Focus(window) => {
                self.working.focus = window.clone();
                self.working.focus_app = window.as_ref().and_then(|w| self.app_of(w));
                self.working.focus_title = window
                    .as_ref()
                    .and_then(|w| self.windows.get(w))
                    .map(|record| record.title.clone())
                    .unwrap_or_default();
                let focused = window.as_ref().and_then(|w| self.focused(w));
                self.segmenter.focus(focused, t)
            }
            ObservationKind::Session(state) => {
                self.session = *state;
                match state {
                    SessionState::Active => self.segmenter.tick(t),
                    SessionState::Idle | SessionState::Locked => self.segmenter.interrupt(t),
                }
            }
            ObservationKind::InputActivity {
                window: Some(window),
                interval,
                counts,
            } => self.segmenter.heartbeat(window, counts, *interval, t),
            ObservationKind::Text {
                window,
                url: Some(url),
                ..
            } => {
                self.urls.insert(window.clone(), url.clone());
                match self.focused(window) {
                    Some(focused) => self.segmenter.document_changed(focused, t),
                    None => self.segmenter.tick(t),
                }
            }
            ObservationKind::Paused { paused, .. } => {
                self.observing = !paused;
                if *paused {
                    self.segmenter.interrupt(t)
                } else {
                    self.segmenter.tick(t)
                }
            }
            _ => self.segmenter.tick(t),
        };
        let mut context_changed = false;
        for event in segment_events {
            match &event {
                SegmentEvent::Opened(segment) | SegmentEvent::Extended(segment) => {
                    self.working.segment = Some(segment.clone());
                    if matches!(event, SegmentEvent::Opened(_)) {
                        self.working.note_document(&segment.doc_key.to_string());
                        context_changed = true;
                    }
                }
                SegmentEvent::Closed(segment) => {
                    let closed = self.finish(segment);
                    if self
                        .working
                        .segment
                        .as_ref()
                        .is_some_and(|open| open.id == closed.id)
                    {
                        self.working.segment = None;
                    }
                    for episode_event in self.episodes.add(&closed) {
                        if let EpisodeEvent::Opened(episode) | EpisodeEvent::Updated(episode) =
                            &episode_event
                        {
                            self.working.episode = Some(episode.clone());
                        }
                        if let EpisodeEvent::Closed(episode) = &episode_event
                            && self
                                .working
                                .episode
                                .as_ref()
                                .is_some_and(|open| open.id == episode.id)
                        {
                            self.working.episode = None;
                        }
                        events.push(EngineEvent::Episode(episode_event));
                    }
                    events.push(EngineEvent::Segment(SegmentEvent::Closed(closed)));
                    context_changed = true;
                    continue;
                }
                SegmentEvent::Transition { .. } => {}
            }
            events.push(EngineEvent::Segment(event));
        }
        for episode_event in self.episodes.tick(t) {
            if let EpisodeEvent::Closed(episode) = &episode_event
                && self
                    .working
                    .episode
                    .as_ref()
                    .is_some_and(|open| open.id == episode.id)
            {
                self.working.episode = None;
            }
            events.push(EngineEvent::Episode(episode_event));
        }
        if context_changed
            || matches!(
                observation.kind,
                ObservationKind::Focus(_) | ObservationKind::Session(_)
            )
        {
            events.push(EngineEvent::ContextChanged(self.current_context(t)));
        }
        events
    }

    fn app_of(&self, window: &WindowIdent) -> Option<AppRef> {
        self.apps.get(window).cloned().or_else(|| {
            self.windows.get(window).map(|record| AppRef {
                app_key: record.app_id.to_ascii_lowercase(),
                app_id: record.app_id.clone(),
                name: record.app_id.clone(),
            })
        })
    }

    fn focused(&self, window: &WindowIdent) -> Option<Focused> {
        let record = self.windows.get(window)?;
        let app = self.app_of(window)?;
        let url = self.urls.get(window).map(String::as_str);
        let doc_key = derive_doc_key(record, url, self.home.as_deref());
        Some(Focused {
            window: window.clone(),
            app_key: app.app_key,
            doc_key: doc_key.clone(),
            title: record.title.clone(),
            url: match &doc_key {
                context_core::DocKey::Url(url) => Some(url.clone()),
                _ => None,
            },
            file_path: match &doc_key {
                context_core::DocKey::File(path) => Some(path.clone()),
                _ => None,
            },
            cwd: match &doc_key {
                context_core::DocKey::Directory(dir) => Some(dir.clone()),
                _ => None,
            },
        })
    }

    /// Labels and scores a segment as it closes.
    fn finish(&self, segment: &Segment) -> Segment {
        let mut closed = segment.clone();
        let app_id = self
            .windows
            .values()
            .find(|record| record.app_id.to_ascii_lowercase() == closed.app_key)
            .map(|record| record.app_id.clone())
            .unwrap_or_else(|| closed.app_key.clone());
        let (label, confidence) = classify(&app_id, closed.url.as_deref(), &closed.title);
        closed.label = label;
        closed.label_confidence = confidence;
        let copies = self
            .correlator
            .copies_between(closed.range.start, closed.range.end);
        closed.importance = importance(&closed, copies);
        closed
    }
}
