//! Where a capture takes its text from, in this order: the window's CrownUI tree, its AT-SPI
//! tree, then OCR. No CrownUI tree reader exists yet, so CrownUI apps are read through AT-SPI.

use std::time::Duration;

use context_capture::{
    Cancellation, CaptureTarget, StructuredLines, StructuredReading, StructuredText,
};
use context_collect::a11y::{AccessibleText, RecentReadings};
use context_core::TextSource;

/// The collector reads at once on a capture request; a read takes up to 200 ms. The frame is
/// zero during this wait.
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

/// Only a reading that started after the capture request counts.
fn reading_of(text: Option<AccessibleText>) -> StructuredReading {
    match text {
        Some(AccessibleText::Lines(lines)) => StructuredReading::Text(StructuredLines {
            source: TextSource::Accessibility,
            lines,
        }),
        Some(AccessibleText::Sensitive) => StructuredReading::Sensitive,
        Some(AccessibleText::Unread) => StructuredReading::Unavailable,
        None => StructuredReading::Pending,
    }
}

impl StructuredText for WindowStructuredText {
    fn window_text(&mut self, target: &CaptureTarget) -> StructuredReading {
        reading_of(
            self.accessibility
                .latest(&target.window, target.requested_at),
        )
    }

    fn wait_for_text(
        &mut self,
        target: &CaptureTarget,
        cancel: &dyn Cancellation,
    ) -> StructuredReading {
        match reading_of(self.accessibility.wait_for(
            &target.window,
            target.requested_at,
            READING_WAIT,
            || cancel.is_cancelled(),
        )) {
            StructuredReading::Pending => StructuredReading::Unavailable,
            reading => reading,
        }
    }
}
