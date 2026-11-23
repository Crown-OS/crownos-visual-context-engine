use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use context_core::WindowIdent;
use crownos_protocols::context::v1::client::crownos_context_window_v1::{
    CrownosContextWindowV1, FlagReason,
};
use wayland_client::Connection;

/// Why the engine asks the compositor to treat a window as sensitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveReason {
    SecretText,
    User,
}

/// Called with `true` before the process becomes readable through `/proc` to connect, and with
/// `false` after. A `false` answer to `true` means frames may still be in memory: the process
/// stays hidden and the bridge connects later.
pub type ExposureHook = Arc<dyn Fn(bool) -> bool + Send + Sync>;

#[derive(Default)]
struct LinkState {
    connection: Option<Connection>,
    windows: BTreeMap<WindowIdent, CrownosContextWindowV1>,
    exposure_hook: Option<ExposureHook>,
}

impl std::fmt::Debug for LinkState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinkState")
            .field("connected", &self.connection.is_some())
            .field("windows", &self.windows.len())
            .finish_non_exhaustive()
    }
}

/// The bridge's live connection and window objects, for the rest of the engine. Requests are
/// sent from the caller's thread and flushed at once, so the bridge thread need not wake.
#[derive(Debug, Clone, Default)]
pub struct BridgeLink {
    state: Arc<Mutex<LinkState>>,
}

impl BridgeLink {
    /// The compositor connection while the bridge is bound.
    pub fn connection(&self) -> Option<Connection> {
        self.state.lock().ok()?.connection.clone()
    }

    /// Asks the compositor to fail captures of `window` for `duration`. Returns whether the
    /// request went out.
    pub fn flag_sensitive(
        &self,
        window: &WindowIdent,
        reason: SensitiveReason,
        duration: Duration,
    ) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        let (Some(connection), Some(proxy)) = (&state.connection, state.windows.get(window)) else {
            return false;
        };
        let reason = match reason {
            SensitiveReason::SecretText => FlagReason::SecretText,
            SensitiveReason::User => FlagReason::User,
        };
        let milliseconds = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
        proxy.flag_sensitive(reason, milliseconds);
        connection.flush().is_ok()
    }

    /// Lets the capture pipeline hold its frames while the bridge connects.
    pub fn set_exposure_hook(&self, hook: ExposureHook) {
        if let Ok(mut state) = self.state.lock() {
            state.exposure_hook = Some(hook);
        }
    }

    pub(super) fn exposure_hook(&self) -> Option<ExposureHook> {
        self.state.lock().ok()?.exposure_hook.clone()
    }

    pub(super) fn connected(&self, connection: Connection) {
        if let Ok(mut state) = self.state.lock() {
            state.connection = Some(connection);
        }
    }

    pub(super) fn disconnected(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.connection = None;
            state.windows.clear();
        }
    }

    pub(super) fn window_described(&self, window: WindowIdent, proxy: CrownosContextWindowV1) {
        if let Ok(mut state) = self.state.lock() {
            state.windows.insert(window, proxy);
        }
    }

    pub(super) fn window_closed(&self, window: &WindowIdent) {
        if let Ok(mut state) = self.state.lock() {
            state.windows.remove(window);
        }
    }
}
