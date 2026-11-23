//! The observation thread: policy, engine, store, and the events the API pushes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use context_capture::{CaptureHold, CaptureScheduler, CaptureService};
use context_collect::BridgeLink;
use context_collect::apps::AppResolver;
use context_core::{
    Observation, ObservationKind, SessionState, Timestamp, WindowIdent, WindowRecord,
};
use context_engine::{EngineEvent, SegmentEvent};
use context_policy::Policy;
use context_protocol::{CurrentContext, SegmentView};
use context_store::{Write, WriteHandle};
use tokio::sync::mpsc::UnboundedSender;

use crate::capture;
use crate::capture_output::CaptureOutput;
use crate::local_sensitivity::LocalSensitivity;
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
    /// Waits until the observation channel closes and every observation in it is handled.
    #[cfg(test)]
    pub(crate) fn drain(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the pipeline captures with, when the compositor is live.
pub(crate) struct Capturing {
    pub(crate) service: Arc<CaptureService>,
    pub(crate) link: BridgeLink,
}

pub(crate) fn spawn(
    observations: Receiver<Observation>,
    policy: Policy,
    writes: WriteHandle,
    shared: Arc<Mutex<Shared>>,
    events: UnboundedSender<Outgoing>,
    capturing: Option<Capturing>,
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
                    apps: AppResolver::default(),
                    last_context: None,
                    pending_context: None,
                    last_snapshot: Instant::now(),
                    capturing,
                    scheduler: CaptureScheduler::default(),
                    capture_output: CaptureOutput::default(),
                    locally_sensitive: LocalSensitivity::default(),
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
    apps: AppResolver,
    last_context: Option<Instant>,
    pending_context: Option<CurrentContext>,
    last_snapshot: Instant,
    capturing: Option<Capturing>,
    scheduler: CaptureScheduler,
    capture_output: CaptureOutput,
    locally_sensitive: LocalSensitivity,
}

impl Worker {
    fn run(&mut self, observations: &Receiver<Observation>, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match observations.recv_timeout(self.wait_for_next()) {
                Ok(observation) => self.observe(observation),
                Err(RecvTimeoutError::Timeout) => self.idle(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.capture_when_due();
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
        if self.holds_secret(&observation) {
            shared.observations_dropped += 1;
            return;
        }
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
        if let ObservationKind::WindowClosed(window) = &observation.kind {
            self.capture_output.forget_window(window);
        }
        self.capture_output.take(&observation);
        self.write_capture_output(&shared);
        self.follow_capture_gates(&observation.kind);
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
    /// sensitive ones.
    fn admit(&self, shared: &Shared, observation: Observation) -> Option<Observation> {
        let verdict = |record: &WindowRecord| self.policy.judge(record);
        match &observation.kind {
            ObservationKind::WindowOpened(record) | ObservationKind::WindowChanged(record) => {
                verdict(record).allows_metadata().then_some(observation)
            }
            ObservationKind::Text { window, .. }
            | ObservationKind::Frame(context_core::FrameRecord { window, .. }) => {
                let allowed = shared
                    .engine
                    .window(window)
                    .is_some_and(|record| verdict(record).allows_text());
                allowed.then_some(observation)
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
            EngineEvent::Capture(due) => {
                if self.capturing.is_some() {
                    self.scheduler.request(due.trigger, due.window, due.at);
                }
            }
            EngineEvent::ContextChanged(context) => self.pending_context = Some(context),
        }
    }

    fn wait_for_next(&self) -> Duration {
        self.scheduler.next_due().map_or(IDLE_TICK, |due| {
            let wait = due.since(Timestamp::now());
            Duration::from_micros(u64::try_from(wait.0).unwrap_or(0)).min(IDLE_TICK)
        })
    }

    /// Takes the capture that is due, if every gate still passes now.
    fn capture_when_due(&mut self) {
        let Some(capturing) = &self.capturing else {
            return;
        };
        let Some((trigger, window)) = self.scheduler.take_due(Timestamp::now()) else {
            return;
        };
        let Ok(shared) = self.shared.lock() else {
            return;
        };
        self.locally_sensitive.forget_expired();
        let target = capture::target(
            &shared,
            &self.policy,
            &self.locally_sensitive,
            trigger,
            window.as_ref(),
        );
        match target {
            Some(target) => capturing.service.capture(target),
            None => tracing::debug!(?trigger, "a capture was due but a gate refused it"),
        }
    }

    /// Writes the capture output whose segment is open, when the policy still allows text from
    /// its window: the gates can change between the capture and the store.
    fn write_capture_output(&mut self, shared: &Shared) {
        let still_allowed = |window: &WindowIdent| {
            shared.enabled
                && !shared.paused
                && shared.engine.is_observing()
                && !self.locally_sensitive.is_marked(window)
                && shared
                    .engine
                    .window(window)
                    .is_some_and(|record| self.policy.judge(record).allows_text())
        };
        for write in self
            .capture_output
            .ready_writes(&shared.engine, still_allowed)
        {
            let _ = self.writes.send(write);
        }
    }

    /// A focus change ends the capture session of the old window. A pause or a lock holds all
    /// captures, drops the queued frames and the output that waits for a segment.
    fn follow_capture_gates(&mut self, kind: &ObservationKind) {
        let Some(capturing) = &self.capturing else {
            return;
        };
        let service = &capturing.service;
        match kind {
            ObservationKind::Focus(_) => service.end_session(),
            ObservationKind::Paused { paused: true, .. } => {
                service.suspend(CaptureHold::Paused, Duration::ZERO);
            }
            ObservationKind::Paused { paused: false, .. } => service.resume(CaptureHold::Paused),
            ObservationKind::Session(SessionState::Locked) => {
                service.suspend(CaptureHold::Locked, Duration::ZERO);
            }
            ObservationKind::Session(SessionState::Active | SessionState::Idle) => {
                service.resume(CaptureHold::Locked);
            }
            _ => return,
        }
        if matches!(
            kind,
            ObservationKind::Paused { paused: true, .. }
                | ObservationKind::Session(SessionState::Locked)
        ) {
            self.scheduler.clear();
            self.capture_output.clear();
        }
    }

    /// Text with a secret in it, from any source, and the record of a capture that saw one, are
    /// dropped; the window is flagged.
    fn holds_secret(&mut self, observation: &Observation) -> bool {
        let window = match &observation.kind {
            ObservationKind::Text { window, lines, .. }
                if !context_policy::find_secrets(&lines.join("\n")).is_empty() =>
            {
                window
            }
            ObservationKind::Frame(frame) if frame.secret_seen => &frame.window,
            _ => return false,
        };
        self.capture_output.forget_window(window);
        let flagged = self
            .capturing
            .as_ref()
            .is_some_and(|capturing| capture::flag_secret(&capturing.link, window));
        if !flagged {
            tracing::info!("the compositor did not take the flag; the engine holds the window");
        }
        self.locally_sensitive
            .mark(window, capture::SECRET_FLAG_DURATION);
        true
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
        self.write_capture_output(&shared);
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

/// Input buckets live on in their segment's profile, text and frames in their own tables, and a
/// copy the source marked secret nowhere; everything else is an event row.
const fn persisted(kind: &ObservationKind) -> bool {
    !matches!(
        kind,
        ObservationKind::InputActivity { .. }
            | ObservationKind::Text { .. }
            | ObservationKind::Frame(_)
            | ObservationKind::Selection { secret: true, .. }
    )
}
