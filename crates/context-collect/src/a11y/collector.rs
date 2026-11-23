use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use atspi::ObjectRefOwned;
use context_core::{Observation, ObservationKind, TextSource, Timestamp, WindowIdent};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::A11yError;
use super::applications::{ApplicationDirectory, root_of};
use super::backoff::AppBackoff;
use super::dedup::TextDeduplicator;
use super::events::{AccessibilityEvent, EventKind};
use super::health::ObjectHealth;
use super::host::AccessibilityHost;
use super::read::{FrameContent, active_frame, read_frame};
use super::reader::{ReadError, Reader};
use super::schedule::{ReadSchedule, Wake};
use super::window_match::match_window;
use crate::Outlet;
use crate::apps::AppResolver;

/// The whole budget of one read of one application.
const READ_BUDGET: Duration = Duration::from_millis(200);

pub(crate) struct Collector<'h, H> {
    connection: zbus::Connection,
    host: &'h H,
    outlet: Outlet,
    health: RefCell<ObjectHealth>,
    applications: ApplicationDirectory,
    resolver: AppResolver,
    activated_frames: HashMap<String, ObjectRefOwned>,
    backoff: HashMap<String, AppBackoff>,
    schedule: ReadSchedule,
    dedup: TextDeduplicator,
    focused_application: Option<String>,
}

impl<H> std::fmt::Debug for Collector<'_, H> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Collector")
            .field("focused_application", &self.focused_application)
            .finish_non_exhaustive()
    }
}

impl<'h, H: AccessibilityHost> Collector<'h, H> {
    pub(crate) fn new(connection: zbus::Connection, host: &'h H, outlet: Outlet) -> Self {
        Self {
            connection,
            host,
            outlet,
            health: RefCell::default(),
            applications: ApplicationDirectory::default(),
            resolver: AppResolver::default(),
            activated_frames: HashMap::new(),
            backoff: HashMap::new(),
            schedule: ReadSchedule::default(),
            dedup: TextDeduplicator::default(),
            focused_application: None,
        }
    }

    pub(crate) async fn run(
        mut self,
        mut events: mpsc::Receiver<AccessibilityEvent>,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<(), A11yError> {
        let mut focus = self.host.watch_focus();
        let focused = focus.borrow_and_update().clone();
        self.on_compositor_focus(focused).await;
        loop {
            let due = self.schedule.due();
            tokio::select! {
                _ = stop.changed() => return Ok(()),
                event = events.recv() => match event {
                    Some(event) => self.on_event(&event),
                    None => return Err(A11yError::BusClosed),
                },
                changed = focus.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    let focused = focus.borrow_and_update().clone();
                    self.on_compositor_focus(focused).await;
                }
                () = sleep_until_due(due) => self.read_due().await,
            }
        }
    }

    fn on_event(&mut self, event: &AccessibilityEvent) {
        let Some(application) = event.application() else {
            return;
        };
        let now = Instant::now();
        let is_focused = self.focused_application.as_deref() == Some(application);
        let wake = match event.kind {
            EventKind::Focused => Wake::FocusChange,
            EventKind::WindowActivated => {
                self.activated_frames
                    .insert(application.to_owned(), event.source.clone());
                Wake::FocusChange
            }
            EventKind::DocumentLoaded if is_focused => Wake::DocumentLoaded,
            EventKind::NameChanged if is_focused => Wake::TitleChange,
            EventKind::TextChanged if is_focused => Wake::Typing,
            EventKind::DocumentLoaded | EventKind::NameChanged | EventKind::TextChanged => return,
        };
        if wake == Wake::FocusChange {
            self.focused_application = Some(application.to_owned());
        }
        self.schedule.wake(application, wake, now);
    }

    /// Covers toolkits that send no focus event of their own.
    async fn on_compositor_focus(&mut self, window: Option<WindowIdent>) {
        let record = window.and_then(|ident| {
            self.host
                .with_windows(|windows| windows.get(&ident).cloned())
        });
        let Some(record) = record else {
            self.focused_application = None;
            return;
        };
        let application = self
            .applications
            .find_for(&self.connection, &self.health, &mut self.resolver, &record)
            .await;
        if let Some(application) = application {
            self.activated_frames.remove(&application);
            self.schedule
                .wake(&application, Wake::FocusChange, Instant::now());
            self.focused_application = Some(application);
        }
    }

    async fn read_due(&mut self) {
        let now = Instant::now();
        let Some(application) = self.schedule.take_due(now) else {
            return;
        };
        let backoff = self
            .backoff
            .entry(application.clone())
            .or_insert_with(|| AppBackoff::new(now));
        if backoff.ready_at() > now {
            let until = backoff.ready_at();
            self.schedule.defer(application, until);
            return;
        }
        let outcome = self.read(&application).await;
        let cut_short = match &outcome {
            Ok(complete) => !complete,
            Err(ReadError::OutOfTime) => true,
            Err(ReadError::Gone) => {
                self.activated_frames.remove(&application);
                self.applications.forget(&application);
                self.health.borrow_mut().forget_application(&application);
                false
            }
            Err(error) => {
                tracing::debug!(%error, application, "accessibility read failed");
                false
            }
        };
        if let Some(backoff) = self.backoff.get_mut(&application) {
            backoff.record(now.elapsed(), cut_short, Instant::now());
        }
    }

    /// Whether the read was complete.
    async fn read(&mut self, application: &str) -> Result<bool, ReadError> {
        let info = self
            .applications
            .describe(&self.connection, &self.health, application)
            .await
            .ok_or(ReadError::Gone)?;
        let root = root_of(application).ok_or(ReadError::Gone)?;
        let reader = Reader::new(
            &self.connection,
            info.toolkit,
            &self.health,
            Instant::now() + READ_BUDGET,
        );
        let frame = match self.activated_frames.get(application) {
            Some(frame) => frame.clone(),
            None => active_frame(&reader, &root).await?.ok_or(ReadError::Gone)?,
        };
        let title = reader.name(&frame).await?;
        let resolver = &mut self.resolver;
        let record = self.host.with_windows(|windows| {
            match_window(windows, resolver, info.pid, &title)
                .and_then(|ident| windows.get(&ident).cloned())
        });
        let Some(record) = record else {
            return Ok(true);
        };
        if !self.host.may_read(&record) {
            return Ok(true);
        }
        match read_frame(&reader, &root, &frame).await? {
            FrameContent::PasswordField => {
                self.host.flag_sensitive(&record.ident);
                Ok(true)
            }
            FrameContent::Text {
                lines,
                url,
                complete,
                ..
            } => {
                if !lines.is_empty() && self.dedup.is_new(&record.ident, &lines, Instant::now()) {
                    self.emit(record.ident, url, lines);
                }
                Ok(complete)
            }
        }
    }

    fn emit(&self, window: WindowIdent, url: Option<String>, lines: Vec<String>) {
        let observation = Observation::new(
            Timestamp::now(),
            ObservationKind::Text {
                window,
                source: TextSource::Accessibility,
                url,
                lines,
            },
        );
        if self.outlet.try_send(observation).is_err() {
            tracing::debug!("the observation queue is full; accessibility text dropped");
        }
    }
}

async fn sleep_until_due(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}
