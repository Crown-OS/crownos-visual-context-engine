use serde::{Deserialize, Serialize};

use crate::time::{Micros, Timestamp};
use crate::window::{WindowIdent, WindowRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Active,
    Idle,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionKind {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PauseOrigin {
    Hotkey,
    Engine,
    Lock,
}

/// Where a line of text came from, in order of trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TextSource {
    CrownUi,
    Accessibility,
    Ocr,
    Vision,
}

/// What the user did in one interval, as counts and never as content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InputCounts {
    pub text_keys: u32,
    pub edit_keys: u32,
    pub nav_keys: u32,
    pub chords: u32,
    pub clicks: u32,
    pub scroll_ticks: u32,
    pub pointer_travel: u32,
    pub typing_paused: bool,
    pub scroll_stopped: bool,
}

impl InputCounts {
    pub const fn total_keys(&self) -> u32 {
        self.text_keys + self.edit_keys + self.nav_keys + self.chords
    }

    pub const fn is_empty(&self) -> bool {
        self.total_keys() == 0
            && self.clicks == 0
            && self.scroll_ticks == 0
            && self.pointer_travel == 0
    }
}

/// One thing a collector saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationKind {
    WindowOpened(WindowRecord),
    WindowChanged(WindowRecord),
    WindowClosed(WindowIdent),
    Focus(Option<WindowIdent>),
    Session(SessionState),
    InputActivity {
        window: Option<WindowIdent>,
        interval: Micros,
        counts: InputCounts,
    },
    Selection {
        kind: SelectionKind,
        window: Option<WindowIdent>,
        mime_types: Vec<String>,
        size_hint: u32,
        secret: bool,
    },
    Paused {
        paused: bool,
        origin: PauseOrigin,
    },
    /// Text on screen, from the accessibility tree, the CrownUI tree or OCR.
    Text {
        window: WindowIdent,
        source: TextSource,
        url: Option<String>,
        lines: Vec<String>,
    },
    /// A notification was posted; the body stays with the notifier.
    Notification {
        app_id: String,
        category: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub t: Timestamp,
    pub kind: ObservationKind,
}

impl Observation {
    pub const fn new(t: Timestamp, kind: ObservationKind) -> Self {
        Self { t, kind }
    }

    /// The window the observation is about, when it is about one.
    pub const fn window(&self) -> Option<&WindowIdent> {
        match &self.kind {
            ObservationKind::WindowOpened(record) | ObservationKind::WindowChanged(record) => {
                Some(&record.ident)
            }
            ObservationKind::WindowClosed(ident) | ObservationKind::Text { window: ident, .. } => {
                Some(ident)
            }
            ObservationKind::Focus(window)
            | ObservationKind::InputActivity { window, .. }
            | ObservationKind::Selection { window, .. } => window.as_ref(),
            ObservationKind::Session(_)
            | ObservationKind::Paused { .. }
            | ObservationKind::Notification { .. } => None,
        }
    }
}

/// Why a capture of the focused window is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Trigger {
    FocusChange,
    TitleChange,
    TypingPause,
    ScrollStop,
    Copy,
    DamageSettled,
    IdleBackstop,
}

impl Trigger {
    /// How long to wait after the trigger before capturing, so a flicker costs nothing.
    pub const fn settle(self) -> Micros {
        match self {
            Self::FocusChange | Self::TitleChange => Micros::from_millis(400),
            Self::TypingPause => Micros::from_millis(1_500),
            Self::ScrollStop => Micros::from_millis(500),
            Self::Copy => Micros::ZERO,
            Self::DamageSettled => Micros::from_millis(750),
            Self::IdleBackstop => Micros::from_secs(60),
        }
    }
}
