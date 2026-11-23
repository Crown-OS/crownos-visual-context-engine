//! The observation thread: policy, engine, store, and the events the API pushes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use context_collect::CompositorLink;
use context_collect::apps::AppResolver;
use context_core::{Observation, ObservationKind, Timestamp, WindowRecord};
use context_engine::{EngineEvent, SegmentEvent};
use context_policy::{Policy, find_secrets};
use context_protocol::{CurrentContext, SegmentView};
use context_store::{Write, WriteHandle};
use tokio::sync::mpsc::UnboundedSender;

use crate::shared::Shared;

/// `ContextChanged` goes out at most this often.
const CONTEXT_INTERVAL: Duration = Duration::from_millis(250);
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(60);
const IDLE_TICK: Duration = Duration::from_secs(5);

/// What the API pushes to subscribers.
#[derive(Debug)]
pub(crate) enum Outgoing {
    Context(CurrentContext),
    Segment(SegmentView),
}

pub(crate) struct Pipeline {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pipeline {
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn spawn(
    observations: Receiver<Observation>,
    policy: Arc<Policy>,
    writes: WriteHandle,
    shared: Arc<Mutex<Shared>>,
    events: UnboundedSender<Outgoing>,
    link: CompositorLink,
) -> Result<Pipeline> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread = std::thread::Builder::new()
        .name("context-pipeline".to_owned())
        .spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut worker = Worker {
                    policy,
                    writes,
                    shared,
                    events,
                    link,
                    apps: AppResolver::default(),
                    last_context: None,
                    pending_context: None,
                    last_snapshot: Instant::now(),
                };
                worker.run(&observations, &stop);
            }
        })?;
    Ok(Pipeline {
        stop,
        thread: Some(thread),
    })
}

struct Worker {
    policy: Arc<Policy>,
    writes: WriteHandle,
    shared: Arc<Mutex<Shared>>,
    events: UnboundedSender<Outgoing>,
    link: CompositorLink,
    apps: AppResolver,
    last_context: Option<Instant>,
    pending_context: Option<CurrentContext>,
    last_snapshot: Instant,
}

impl Worker {
    fn run(&mut self, observations: &Receiver<Observation>, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match observations.recv_timeout(IDLE_TICK) {
                Ok(observation) => self.observe(observation),
                Err(RecvTimeoutError::Timeout) => self.idle(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.flush_context(false);
        }
        self.flush_context(true);
        self.snapshot();
    }

    fn observe(&mut self, observation: Observation) {
        let shows_secret = shows_secret(&observation.kind);
        let shared = Arc::clone(&self.shared);
        let Ok(mut shared) = shared.lock() else {
            return;
        };
        shared.observations_seen += 1;
        if shared.paused && !matches!(observation.kind, ObservationKind::Paused { .. }) {
            shared.observations_dropped += 1;
            return;
        }
        let Some(observation) = self.admit(&shared, observation, shows_secret) else {
            shared.observations_dropped += 1;
            return;
        };
        if let ObservationKind::WindowOpened(record) | ObservationKind::WindowChanged(record) =
            &observation.kind
        {
            let app = self.apps.resolve(&record.app_id, record.pid);
            let _ = self.writes.send(Write::App {
                app_key: app.app_key.clone(),
                app_id: app.app_id.clone(),
                name: app.name.clone(),
            });
            shared.engine.set_app(&record.ident, app);
        }
        if let ObservationKind::WindowClosed(ident) = &observation.kind
            && let Some(record) = shared.engine.window(ident)
        {
            self.apps.forget_pid(record.pid);
        }
        let app_key = observation
            .window()
            .and_then(|ident| shared.engine.window(ident))
            .map(|record| record.app_id.to_ascii_lowercase());
        if persisted(&observation.kind) {
            let _ = self.writes.send(Write::Event {
                observation: observation.clone(),
                app_key,
                t_end: observation.t,
            });
        }
        for event in shared.engine.ingest(&observation) {
            self.handle(event);
        }
        if self.last_snapshot.elapsed() >= SNAPSHOT_INTERVAL {
            if let Ok(snapshot) = shared.engine.working_memory().snapshot() {
                let _ = self.writes.send(Write::WorkingMemory {
                    snapshot,
                    t: Timestamp::now(),
                });
            }
            self.last_snapshot = Instant::now();
        }
    }

    /// The policy's say: whole observations dropped for excluded windows, text stripped for
    /// sensitive ones, and text with a secret in it dropped while its window is flagged.
    fn admit(
        &self,
        shared: &Shared,
        observation: Observation,
        shows_secret: bool,
    ) -> Option<Observation> {
        let verdict = |record: &WindowRecord| self.policy.judge(record);
        match &observation.kind {
            ObservationKind::WindowOpened(record) | ObservationKind::WindowChanged(record) => {
                verdict(record).allows_metadata().then_some(observation)
            }
            ObservationKind::Text { window, .. } => {
                let allowed = shared
                    .engine
                    .window(window)
                    .is_some_and(|record| verdict(record).allows_text());
                if !allowed {
                    return None;
                }
                if shows_secret {
                    self.link.flag_sensitive(window);
                    return None;
                }
                Some(observation)
            }
            ObservationKind::Focus(Some(window))
            | ObservationKind::InputActivity {
                window: Some(window),
                ..
            } => {
                let known = shared.engine.window(window).is_some();
                known.then_some(observation)
            }
            _ => Some(observation),
        }
    }

    fn handle(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Segment(
                SegmentEvent::Opened(segment) | SegmentEvent::Extended(segment),
            ) => {
                let _ = self.writes.send(Write::Segment(Box::new(segment)));
            }
            EngineEvent::Segment(SegmentEvent::Closed(segment)) => {
                let view = SegmentView::from(&segment);
                let _ = self.writes.send(Write::Segment(Box::new(segment)));
                let _ = self.events.send(Outgoing::Segment(view));
            }
            EngineEvent::Segment(SegmentEvent::Transition { .. }) => {}
            EngineEvent::Episode(
                context_engine::EpisodeEvent::Opened(episode)
                | context_engine::EpisodeEvent::Updated(episode)
                | context_engine::EpisodeEvent::Closed(episode),
            ) => {
                let _ = self.writes.send(Write::Episode(Box::new(episode)));
            }
            EngineEvent::Capture(_) => {}
            EngineEvent::ContextChanged(context) => self.pending_context = Some(context),
        }
    }

    fn idle(&mut self) {
        let now = Timestamp::now();
        let shared = Arc::clone(&self.shared);
        let Ok(mut shared) = shared.lock() else {
            return;
        };
        let tick = Observation::new(
            now,
            ObservationKind::Session(context_core::SessionState::Active),
        );
        if shared.engine.is_observing() {
            for event in shared.engine.ingest(&tick) {
                self.handle(event);
            }
        }
    }

    fn flush_context(&mut self, force: bool) {
        let due = self
            .last_context
            .is_none_or(|last| last.elapsed() >= CONTEXT_INTERVAL);
        if (due || force)
            && let Some(context) = self.pending_context.take()
        {
            let _ = self.events.send(Outgoing::Context(context));
            self.last_context = Some(Instant::now());
        }
    }

    fn snapshot(&self) {
        if let Ok(shared) = self.shared.lock()
            && let Ok(snapshot) = shared.engine.working_memory().snapshot()
        {
            let _ = self.writes.send(Write::WorkingMemory {
                snapshot,
                t: Timestamp::now(),
            });
        }
    }
}

/// Whether text holds a secret, in its lines or its URL. Scanned before the shared state is
/// locked, so the API never waits on the scan.
fn shows_secret(kind: &ObservationKind) -> bool {
    let ObservationKind::Text { lines, url, .. } = kind else {
        return false;
    };
    lines
        .iter()
        .chain(url)
        .any(|text| !find_secrets(text).is_empty())
}

/// Input buckets live on in their segment's profile; everything else is an event row.
const fn persisted(kind: &ObservationKind) -> bool {
    !matches!(
        kind,
        ObservationKind::InputActivity { .. } | ObservationKind::Text { .. }
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use context_core::{TextSource, WindowIdent};
    use context_policy::VisualContext;
    use context_store::{Store, Writer};

    use super::*;

    const AWS_KEY: &str = "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
    const JWT_URL: &str = "https://example.org/callback?token=eyJhbGciOiJIUzI1NiJ9.\
        eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";

    fn text(lines: &[&str], url: Option<&str>) -> Observation {
        Observation::new(
            Timestamp::now(),
            ObservationKind::Text {
                window: WindowIdent("w".into()),
                source: TextSource::Accessibility,
                url: url.map(str::to_owned),
                lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            },
        )
    }

    #[test]
    fn secrets_are_found_in_lines_and_in_the_url() {
        assert!(shows_secret(&text(&[AWS_KEY], None).kind));
        assert!(shows_secret(&text(&["plain"], Some(JWT_URL)).kind));
        assert!(!shows_secret(
            &text(&["plain notes"], Some("https://example.org/")).kind
        ));
    }

    #[test]
    fn admitted_text_with_a_secret_is_dropped() {
        let state_dir =
            std::env::temp_dir().join(format!("crown-contextd-pipeline-{}", std::process::id()));
        let writer = Writer::spawn(Store::open_dir(&state_dir).unwrap()).unwrap();
        let section = VisualContext {
            enabled: true,
            ..VisualContext::default()
        };
        let shared = Arc::new(Mutex::new(Shared::new(true)));
        let (events, _outbox) = tokio::sync::mpsc::unbounded_channel();
        let mut worker = Worker {
            policy: Arc::new(Policy::from_section(&section).unwrap()),
            writes: writer.handle(),
            shared: Arc::clone(&shared),
            events,
            link: CompositorLink::default(),
            apps: AppResolver::default(),
            last_context: None,
            pending_context: None,
            last_snapshot: Instant::now(),
        };
        worker.observe(Observation::new(
            Timestamp::now(),
            ObservationKind::WindowOpened(WindowRecord {
                ident: WindowIdent("w".into()),
                app_id: "org.gnome.TextEditor".into(),
                title: "notes.txt".into(),
                ..WindowRecord::default()
            }),
        ));
        let dropped = || shared.lock().unwrap().observations_dropped;
        worker.observe(text(&["plain notes"], None));
        assert_eq!(dropped(), 0);
        worker.observe(text(&[AWS_KEY], None));
        assert_eq!(dropped(), 1);
        worker.observe(text(&["plain notes"], Some(JWT_URL)));
        assert_eq!(dropped(), 2);
        drop(worker);
        drop(writer);
        let _ = std::fs::remove_dir_all(state_dir);
    }
}
