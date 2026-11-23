//! Where observations come from. The compositor bridge binds `crownos_context_v1` on its own
//! thread and turns its events into [`Observation`]s; the window table keeps every window the
//! compositor described; the app resolver turns a pid into the application behind it; the
//! accessibility collector reads the text of the focused window from the AT-SPI bus. Replaying
//! a synthetic trace stands in for the compositor in tests and in development.
//!
//! [`Observation`]: context_core::Observation

pub mod a11y;
pub mod apps;
mod bridge;
mod replay;
mod util;
mod window_table;

pub use bridge::{
    BridgeError, BridgeHandle, CompositorBridge, CompositorLink, ExposureHook, ExposureRefused,
    Outlet, SensitiveReason,
};
pub use replay::replay;
pub use util::identifiable_connect::connect_identifiable;
pub use window_table::WindowTable;
