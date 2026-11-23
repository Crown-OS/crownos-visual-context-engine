//! Text a window gives without OCR: its CrownUI tree or its accessibility tree. When it has
//! that text, the capture does not run OCR.

use context_core::{TextSource, WindowIdent};

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
    /// No structured text: OCR reads the window.
    Unavailable,
}

pub trait StructuredText: Send {
    fn window_text(&mut self, window: &WindowIdent) -> StructuredReading;
}

/// Every window goes to OCR.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoStructuredText;

impl StructuredText for NoStructuredText {
    fn window_text(&mut self, _: &WindowIdent) -> StructuredReading {
        StructuredReading::Unavailable
    }
}
