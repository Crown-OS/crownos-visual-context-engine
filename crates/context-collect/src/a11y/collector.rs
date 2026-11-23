use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use atspi::ObjectRefOwned;
use context_core::{Observation, ObservationKind, TextSource, Timestamp, WindowIdent};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::applications::{ApplicationDirectory, root_of};
use super::backoff::AppBackoff;
use super::dedup::TextDeduplicator;
use super::events::{AccessibilityEvent, EventKind};
use super::health::ObjectHealth;
use super::read::{FrameContent, active_frame, read_frame};
use super::reader::{AccessibilityBusConnection, ReadError};
use super::recent_readings::{AccessibleText, RecentReadings};
use super::schedule::{ReadSchedule, Wake};
use super::window_match::match_window;
use super::{A11yError, AccessibilityHost};
use crate::Outlet;
use crate::apps::AppResolver;

/// The whole budget of one read of one application.
const READ_BUDGET: Duration = Duration::from_millis(200);
/// Past this many applications, per-application state is pruned.
const TRACKED_APPLICATIONS: usize = 64;

pub(crate) struct Collector<'h, H> {
    bus: AccessibilityBusConnection,
    host: &'h H,
    outlet: Outlet,
    readings: RecentReadings,
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
    pub(crate) fn new(
        bus: AccessibilityBusConnection,
        host: &'h H,
        outlet: Outlet,
        readings: RecentReadings,
    ) -> Self {
        Self {
            bus,
            host,
            outlet,
            readings,
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
                    Some(event) => self.on_event(&event).await,
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

    async fn on_event(&mut self, event: &AccessibilityEvent) {
        let Some(application) = event.application() else {
            return;
        };
        let is_focused = self.focused_application.as_deref() == Some(application);
        let wake = match event.kind {
            EventKind::Focused | EventKind::WindowActivated => {
                if !self.drew_the_focused_window(application).await {
                    return;
                }
                if event.kind == EventKind::WindowActivated {
                    self.activated_frames
                        .insert(application.to_owned(), event.source.clone());
                }
                self.focused_application = Some(application.to_owned());
                Wake::FocusChange
            }
            EventKind::DocumentLoaded if is_focused => Wake::DocumentLoaded,
            EventKind::NameChanged if is_focused => Wake::TitleChange,
            EventKind::TextChanged if is_focused => Wake::Typing,
            EventKind::DocumentLoaded | EventKind::NameChanged | EventKind::TextChanged => return,
        };
        self.schedule.wake(application, wake, Instant::now());
    }

    /// An application may claim focus only for the window the compositor focused: its own pid,
    /// or the same app as the cgroup names it.
    async fn drew_the_focused_window(&mut self, application: &str) -> bool {
        let Some((focused_pid, focused_app_id)) = self.host.with_windows(|windows| {
            windows
                .focus()
                .map(|window| (window.pid, window.app_id.clone()))
        }) else {
            return false;
        };
        let pid = self
            .applications
            .describe(&self.bus, &self.health, application)
            .await
            .pid;
        if pid <= 0 || focused_pid <= 0 {
            return false;
        }
        pid == focused_pid
            || self.resolver.resolve("", pid).app_key
                == self.resolver.resolve(&focused_app_id, focused_pid).app_key
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
            .find_for(&self.bus, &self.health, &mut self.resolver, &record)
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
                self.forget(&application);
                return;
            }
            Err(error) => {
                tracing::debug!(%error, application, "accessibility read failed");
                false
            }
        };
        if let Some(backoff) = self.backoff.get_mut(&application) {
            backoff.record(now.elapsed(), cut_short, Instant::now());
        }
        self.prune(Instant::now());
    }

    fn forget(&mut self, application: &str) {
        self.activated_frames.remove(application);
        self.backoff.remove(application);
        self.applications.forget(application);
        self.health.borrow_mut().forget_application(application);
    }

    /// Keeps per-application state bounded: a backoff that has run out is the same as none, and
    /// only the focused application's frame is still useful.
    fn prune(&mut self, now: Instant) {
        if self.backoff.len() > TRACKED_APPLICATIONS {
            self.backoff.retain(|_, backoff| backoff.ready_at() > now);
        }
        if self.activated_frames.len() > TRACKED_APPLICATIONS {
            let focused = self.focused_application.as_deref();
            self.activated_frames
                .retain(|application, _| Some(application.as_str()) == focused);
        }
    }

    /// Whether the read was complete.
    async fn read(&mut self, application: &str) -> Result<bool, ReadError> {
        let info = self
            .applications
            .describe(&self.bus, &self.health, application)
            .await;
        let root = root_of(application).ok_or(ReadError::Gone)?;
        let reader = self
            .bus
            .reader(info.toolkit, &self.health, Instant::now() + READ_BUDGET);
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
        if !self.host.may_read(&record, &title) {
            self.readings.forget(&record.ident);
            return Ok(true);
        }
        let content = match read_frame(&reader, &root, &frame).await {
            Ok(content) => content,
            Err(error) => {
                self.readings.forget(&record.ident);
                return Err(error);
            }
        };
        match content {
            FrameContent::Sensitive => {
                self.readings
                    .record(&record.ident, AccessibleText::Sensitive);
                self.host.flag_sensitive(&record.ident);
                Ok(true)
            }
            FrameContent::Incomplete => {
                self.readings.record(&record.ident, AccessibleText::Unread);
                Ok(false)
            }
            FrameContent::Text {
                lines,
                url,
                complete,
                ..
            } => {
                if !may_send(self.host, &record.ident, &title) {
                    self.readings.forget(&record.ident);
                    return Ok(complete);
                }
                let text = if complete && !lines.is_empty() {
                    AccessibleText::Lines(lines.clone())
                } else {
                    AccessibleText::Unread
                };
                self.readings.record(&record.ident, text);
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

/// Asked again once the text is read: the window may have closed, turned sensitive or lost the
/// focus to another window while the read ran, or the session may have locked or paused.
fn may_send<H: AccessibilityHost>(host: &H, window: &WindowIdent, frame_title: &str) -> bool {
    let current = host.with_windows(|windows| {
        let focus_elsewhere = windows
            .focus()
            .is_some_and(|focused| &focused.ident != window);
        windows.get(window).filter(|_| !focus_elsewhere).cloned()
    });
    current.is_some_and(|record| host.may_read(&record, frame_title))
}

async fn sleep_until_due(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use context_core::WindowRecord;

    use super::*;
    use crate::WindowTable;

    struct FakeHost {
        windows: WindowTable,
        allowed: Cell<bool>,
    }

    impl AccessibilityHost for FakeHost {
        fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R {
            read(&self.windows)
        }

        fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>> {
            watch::Sender::new(None).subscribe()
        }

        fn may_read(&self, _: &WindowRecord, frame_title: &str) -> bool {
            self.allowed.get() && !frame_title.contains("Private")
        }

        fn flag_sensitive(&self, _: &WindowIdent) {}
    }

    fn host_with(windows: &[&str], focused: Option<&str>) -> FakeHost {
        let mut table = WindowTable::default();
        for ident in windows {
            table.upsert(WindowRecord {
                ident: WindowIdent((*ident).into()),
                ..WindowRecord::default()
            });
        }
        table.set_focus(focused.map(|ident| WindowIdent(ident.into())));
        FakeHost {
            windows: table,
            allowed: Cell::new(true),
        }
    }

    #[test]
    fn text_goes_out_only_while_the_window_may_still_be_read() {
        let window = WindowIdent("a".into());
        let host = host_with(&["a"], Some("a"));
        assert!(may_send(&host, &window, "notes"));
        assert!(!may_send(&host, &window, "Private notes"));
        host.allowed.set(false);
        assert!(!may_send(&host, &window, "notes"));
    }

    #[test]
    fn text_is_dropped_when_the_window_closed_or_lost_the_focus() {
        let window = WindowIdent("a".into());
        assert!(!may_send(&host_with(&["b"], Some("b")), &window, "notes"));
        assert!(!may_send(
            &host_with(&["a", "b"], Some("b")),
            &window,
            "notes"
        ));
        assert!(may_send(&host_with(&["a", "b"], None), &window, "notes"));
    }
}
