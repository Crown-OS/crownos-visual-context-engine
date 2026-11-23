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
    policy: Policy,
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
    policy: Policy,
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
        let shared = Arc::clone(&self.shared);
        let Ok(mut shared) = shared.lock() else {
            return;
        };
        shared.observations_seen += 1;
        if shared.paused && !matches!(observation.kind, ObservationKind::Paused { .. }) {
            shared.observations_dropped += 1;
            return;
        }
        let Some(observation) = self.admit(&shared, observation) else {
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
    fn admit(&self, shared: &Shared, observation: Observation) -> Option<Observation> {
        let verdict = |record: &WindowRecord| self.policy.judge(record);
        match &observation.kind {
            ObservationKind::WindowOpened(record) | ObservationKind::WindowChanged(record) => {
                verdict(record).allows_metadata().then_some(observation)
            }
            ObservationKind::Text { window, lines, .. } => {
                let allowed = shared
                    .engine
                    .window(window)
                    .is_some_and(|record| verdict(record).allows_text());
                if !allowed {
                    return None;
                }
                if lines.iter().any(|line| !find_secrets(line).is_empty()) {
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

/// Input buckets live on in their segment's profile; everything else is an event row.
const fn persisted(kind: &ObservationKind) -> bool {
    !matches!(
        kind,
        ObservationKind::InputActivity { .. } | ObservationKind::Text { .. }
    )
}
