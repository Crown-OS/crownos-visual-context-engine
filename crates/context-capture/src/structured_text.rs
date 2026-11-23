//! Text a window gives without OCR: its CrownUI tree or its accessibility tree. When it has
//! that text, the capture does not run OCR.

use context_core::{TextSource, WindowIdent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredLines {
    pub source: TextSource,
    pub lines: Vec<String>,
}

pub trait StructuredText: Send {
    /// The visible text of the window, or `None` when the window does not expose it.
    fn window_text(&mut self, window: &WindowIdent) -> Option<StructuredLines>;
}

/// Until the CrownUI and AT-SPI collectors exist, every window goes to OCR.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoStructuredText;

impl StructuredText for NoStructuredText {
    fn window_text(&mut self, _: &WindowIdent) -> Option<StructuredLines> {
        None
    }
}
