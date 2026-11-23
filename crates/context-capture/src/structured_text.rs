//! Text a window gives without OCR: its CrownUI tree or its accessibility tree. When it has
//! that text, the capture does not run OCR.

use context_core::TextSource;

use crate::cancellation::Cancellation;
use crate::frame::CaptureTarget;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredLines {
    pub source: TextSource,
    pub lines: Vec<String>,
}

/// What a window's structured text says about one capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructuredReading {
    /// All of the window's shown text: no OCR.
    Text(StructuredLines),
    /// The window shows a password field: the frame is dropped unread.
    Sensitive,
    /// A reading newer than the request may still come.
    Pending,
    /// No structured text: OCR reads the window.
    Unavailable,
}

pub trait StructuredText: Send {
    /// What is known now, without a wait. May ask for a new reading and answer `Pending`.
    fn window_text(&mut self, target: &CaptureTarget) -> StructuredReading;

    /// Waits for the reading that `window_text` answered `Pending` for. The capture calls it
    /// only after its frame is zeroed, and it returns when `cancel` fires.
    fn wait_for_text(
        &mut self,
        target: &CaptureTarget,
        cancel: &dyn Cancellation,
    ) -> StructuredReading;
}

/// Every window goes to OCR.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoStructuredText;

impl StructuredText for NoStructuredText {
    fn window_text(&mut self, _: &CaptureTarget) -> StructuredReading {
        StructuredReading::Unavailable
    }

    fn wait_for_text(&mut self, _: &CaptureTarget, _: &dyn Cancellation) -> StructuredReading {
        StructuredReading::Unavailable
    }
}
