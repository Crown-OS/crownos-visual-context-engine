use serde::{Deserialize, Serialize};

/// The identifier a window's `ext_foreign_toplevel_handle_v1` carries, shared by every protocol
/// that names windows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct WindowIdent(pub String);

/// An application as the engine files it: `app_key` is stable across restarts and sandboxes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct AppRef {
    pub app_key: String,
    pub app_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sandbox {
    pub engine: String,
    pub app_id: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WindowState {
    pub fullscreen: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub visible: bool,
    pub activated: bool,
}

impl WindowState {
    /// From the `crownos_context_window_v1.state` bitfield.
    pub const fn from_bits(bits: u32) -> Self {
        Self {
            fullscreen: bits & 1 != 0,
            maximized: bits & 2 != 0,
            minimized: bits & 4 != 0,
            visible: bits & 8 != 0,
            activated: bits & 16 != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContentType {
    #[default]
    None,
    Photo,
    Video,
    Game,
}

impl ContentType {
    pub const fn from_wire(value: u32) -> Self {
        match value {
            1 => Self::Photo,
            2 => Self::Video,
            3 => Self::Game,
            _ => Self::None,
        }
    }

    /// Captures of moving pictures are wasted OCR and unwanted memories.
    pub const fn is_motion(self) -> bool {
        matches!(self, Self::Video | Self::Game)
    }
}

/// Why a window must not be looked at, as the compositor reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sensitivity {
    pub password_input: bool,
    pub private_browsing: bool,
    pub excluded: bool,
    pub app_requested: bool,
    pub engine_flagged: bool,
}

impl Sensitivity {
    pub const fn from_bits(bits: u32) -> Self {
        Self {
            password_input: bits & 1 != 0,
            private_browsing: bits & 2 != 0,
            excluded: bits & 4 != 0,
            app_requested: bits & 8 != 0,
            engine_flagged: bits & 16 != 0,
        }
    }

    pub const fn any(self) -> bool {
        self.password_input
            || self.private_browsing
            || self.excluded
            || self.app_requested
            || self.engine_flagged
    }
}

/// Everything the compositor says about one window, kept current by the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WindowRecord {
    pub ident: WindowIdent,
    pub app_id: String,
    pub title: String,
    pub pid: i32,
    pub sandbox: Option<Sandbox>,
    pub placement: Option<Placement>,
    pub output: String,
    pub workspace: String,
    pub state: WindowState,
    pub content_type: ContentType,
    pub sensitivity: Sensitivity,
}
