//! Where a capture takes its text from, in this order: the window's CrownUI tree, its AT-SPI
//! tree, then OCR. No CrownUI tree reader exists yet, so CrownUI apps are read through AT-SPI.

use std::time::Duration;

use context_capture::{StructuredLines, StructuredReading, StructuredText};
use context_collect::a11y::{AccessibleText, RecentReadings};
use context_core::{TextSource, WindowIdent};

/// An a11y reading older than this may miss what the capture shows, so OCR reads the window.
const READING_MAX_AGE: Duration = Duration::from_secs(5);
/// The a11y collector reads on the same triggers as a capture; a read takes up to 200 ms.
const READING_WAIT: Duration = Duration::from_millis(300);

#[derive(Debug)]
pub(crate) struct WindowStructuredText {
    accessibility: RecentReadings,
}

impl WindowStructuredText {
    pub(crate) const fn new(accessibility: RecentReadings) -> Self {
        Self { accessibility }
    }
}

impl StructuredText for WindowStructuredText {
    fn window_text(&mut self, window: &WindowIdent) -> StructuredReading {
        match self
            .accessibility
            .fresh(window, READING_MAX_AGE, READING_WAIT)
        {
            Some(AccessibleText::Lines(lines)) => StructuredReading::Text(StructuredLines {
                source: TextSource::Accessibility,
                lines,
            }),
            Some(AccessibleText::Sensitive) => StructuredReading::Sensitive,
            Some(AccessibleText::Unread) | None => StructuredReading::Unavailable,
        }
    }
}
