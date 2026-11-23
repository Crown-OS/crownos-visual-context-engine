use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use context_core::WindowIdent;

/// Past this many windows, readings older than [`KEPT_FOR`] are pruned.
const KEPT_WINDOWS: usize = 32;
const KEPT_FOR: Duration = Duration::from_secs(60);

/// What the last a11y read of a window found, as the capture path needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibleText {
    /// All of the window's shown text.
    Lines(Vec<String>),
    /// A password field is shown, or may be: the window is flagged and nothing is read.
    Sensitive,
    /// The read was cut short or found no text, so OCR covers the window.
    Unread,
}

#[derive(Debug, Default)]
struct Readings {
    by_window: HashMap<WindowIdent, (Instant, AccessibleText)>,
}

/// The last a11y reading of each window, shared by the a11y collector, which records them, and
/// the capture path, which reads them instead of running OCR.
#[derive(Debug, Clone, Default)]
pub struct RecentReadings {
    shared: Arc<(Mutex<Readings>, Condvar)>,
}

impl RecentReadings {
    pub(crate) fn record(&self, window: &WindowIdent, text: AccessibleText) {
        let (readings, changed) = &*self.shared;
        let Ok(mut readings) = readings.lock() else {
            return;
        };
        let now = Instant::now();
        if readings.by_window.len() >= KEPT_WINDOWS {
            readings
                .by_window
                .retain(|_, (read_at, _)| now.duration_since(*read_at) < KEPT_FOR);
        }
        readings.by_window.insert(window.clone(), (now, text));
        changed.notify_all();
    }

    pub(crate) fn forget(&self, window: &WindowIdent) {
        if let Ok(mut readings) = self.shared.0.lock() {
            readings.by_window.remove(window);
        }
    }

    /// The reading of `window` taken in the last `max_age`. Without one, waits up to `wait` for
    /// the collector to read the window, because it reads on the same triggers as a capture.
    pub fn fresh(
        &self,
        window: &WindowIdent,
        max_age: Duration,
        wait: Duration,
    ) -> Option<AccessibleText> {
        let (readings, changed) = &*self.shared;
        let fresh = |readings: &Readings| {
            readings
                .by_window
                .get(window)
                .filter(|(read_at, _)| read_at.elapsed() <= max_age)
                .map(|(_, text)| text.clone())
        };
        let readings = readings.lock().ok()?;
        if let Some(text) = fresh(&readings) {
            return Some(text);
        }
        let (readings, _) = changed
            .wait_timeout_while(readings, wait, |readings| fresh(readings).is_none())
            .ok()?;
        fresh(&readings)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn window(name: &str) -> WindowIdent {
        WindowIdent(name.into())
    }

    #[test]
    fn a_reading_is_used_while_fresh_and_only_for_its_window() {
        let readings = RecentReadings::default();
        readings.record(&window("a"), AccessibleText::Sensitive);
        let now = Duration::ZERO;
        assert_eq!(
            readings.fresh(&window("a"), Duration::from_secs(5), now),
            Some(AccessibleText::Sensitive)
        );
        assert_eq!(
            readings.fresh(&window("b"), Duration::from_secs(5), now),
            None
        );
        assert_eq!(readings.fresh(&window("a"), Duration::ZERO, now), None);
        readings.forget(&window("a"));
        assert_eq!(
            readings.fresh(&window("a"), Duration::from_secs(5), now),
            None
        );
    }

    #[test]
    fn a_capture_waits_briefly_for_the_read_of_the_same_trigger() {
        let readings = RecentReadings::default();
        let collector = readings.clone();
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            collector.record(&window("a"), AccessibleText::Lines(vec!["notes".into()]));
        });
        let text = readings.fresh(&window("a"), Duration::from_secs(5), Duration::from_secs(5));
        reader.join().unwrap();
        assert_eq!(text, Some(AccessibleText::Lines(vec!["notes".into()])));
    }
}
