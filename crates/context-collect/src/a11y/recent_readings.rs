use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use context_core::{Micros, Timestamp, WindowIdent};
use tokio::sync::Notify;

/// Past this many windows, readings older than [`KEPT_FOR`] are pruned.
const KEPT_WINDOWS: usize = 32;
const KEPT_FOR: Micros = Micros::from_secs(60);
/// How often a wait looks at its cancel token.
const CANCEL_CHECK: Duration = Duration::from_millis(10);

/// What the last a11y read of a window found, as the capture path needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibleText {
    /// All of the window's shown text.
    Lines(Vec<String>),
    /// A password field is shown, or may be: the window is flagged and nothing is read.
    Sensitive,
    /// The read was cut short, found no text, or the window has no a11y application, so OCR
    /// covers the window.
    Unread,
}

#[derive(Debug, Default)]
struct Readings {
    by_window: HashMap<WindowIdent, (Timestamp, AccessibleText)>,
    requested: Option<WindowIdent>,
}

#[derive(Debug, Default)]
struct Shared {
    readings: Mutex<Readings>,
    recorded: Condvar,
    requested: Notify,
}

/// The last a11y reading of each window, shared by the a11y collector, which records them, and
/// the capture path, which reads them instead of running OCR. The capture path asks for a new
/// reading through [`RecentReadings::request`].
#[derive(Debug, Clone, Default)]
pub struct RecentReadings {
    shared: Arc<Shared>,
}

impl RecentReadings {
    /// Keeps what a read that started at `read_at` found.
    pub(crate) fn record(&self, window: &WindowIdent, text: AccessibleText, read_at: Timestamp) {
        let Ok(mut readings) = self.shared.readings.lock() else {
            return;
        };
        if readings.by_window.len() >= KEPT_WINDOWS {
            readings
                .by_window
                .retain(|_, (kept_at, _)| read_at.0 - kept_at.0 < KEPT_FOR.0);
        }
        readings.by_window.insert(window.clone(), (read_at, text));
        if readings.requested.as_ref() == Some(window) {
            readings.requested = None;
        }
        self.shared.recorded.notify_all();
    }

    /// The window the capture path wants read, once it asks.
    pub(crate) async fn requested(&self) -> Option<WindowIdent> {
        self.shared.requested.notified().await;
        self.shared.readings.lock().ok()?.requested.clone()
    }

    /// The reading of `window` taken at `since` or later, without waiting.
    pub fn latest(&self, window: &WindowIdent, since: Timestamp) -> Option<AccessibleText> {
        fresh(&*self.shared.readings.lock().ok()?, window, since)
    }

    /// Asks the collector to read `window` now.
    pub fn request(&self, window: &WindowIdent) {
        if let Ok(mut readings) = self.shared.readings.lock() {
            readings.requested = Some(window.clone());
        }
        self.shared.requested.notify_one();
    }

    /// Waits up to `wait` for a reading of `window` taken at `since` or later. Stops early when
    /// `cancelled` says so.
    pub fn wait_for(
        &self,
        window: &WindowIdent,
        since: Timestamp,
        wait: Duration,
        cancelled: impl Fn() -> bool,
    ) -> Option<AccessibleText> {
        let deadline = Instant::now() + wait;
        let mut readings = self.shared.readings.lock().ok()?;
        loop {
            if let Some(text) = fresh(&readings, window, since) {
                return Some(text);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || cancelled() {
                return None;
            }
            readings = self
                .shared
                .recorded
                .wait_timeout(readings, left.min(CANCEL_CHECK))
                .ok()?
                .0;
        }
    }
}

fn fresh(readings: &Readings, window: &WindowIdent, since: Timestamp) -> Option<AccessibleText> {
    readings
        .by_window
        .get(window)
        .filter(|(read_at, _)| read_at.0 >= since.0)
        .map(|(_, text)| text.clone())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn window(name: &str) -> WindowIdent {
        WindowIdent(name.into())
    }

    #[test]
    fn only_a_reading_taken_after_the_request_counts() {
        let readings = RecentReadings::default();
        let before = Timestamp::now();
        readings.record(&window("a"), AccessibleText::Sensitive, Timestamp::now());
        assert_eq!(
            readings.latest(&window("a"), before),
            Some(AccessibleText::Sensitive)
        );
        assert_eq!(readings.latest(&window("b"), before), None);
        let later = Timestamp(Timestamp::now().0 + 1);
        assert_eq!(readings.latest(&window("a"), later), None);
    }

    #[test]
    fn a_wait_ends_with_the_reading_it_waits_for() {
        let readings = RecentReadings::default();
        let collector = readings.clone();
        let since = Timestamp::now();
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            collector.record(
                &window("a"),
                AccessibleText::Lines(vec!["notes".into()]),
                Timestamp::now(),
            );
        });
        let text = readings.wait_for(&window("a"), since, Duration::from_secs(5), || false);
        reader.join().unwrap();
        assert_eq!(text, Some(AccessibleText::Lines(vec!["notes".into()])));
    }

    #[test]
    fn a_cancelled_wait_returns_at_once() {
        let readings = RecentReadings::default();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                cancel.store(true, Ordering::Relaxed);
            });
            let text = readings.wait_for(
                &window("a"),
                Timestamp::now(),
                Duration::from_secs(5),
                || cancel.load(Ordering::Relaxed),
            );
            assert_eq!(text, None);
        });
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
