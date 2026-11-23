use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use context_core::WindowIdent;
use crownos_protocols::context::v1::client::crownos_context_manager_v1::Scope;
use crownos_protocols::context::v1::client::crownos_context_window_v1::{
    CrownosContextWindowV1, FlagReason,
};
use tokio::sync::watch;
use wayland_client::Connection;

use crate::window_table::WindowTable;

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

/// What the bridge thread shares with the other collectors and the capture pipeline: the
/// windows it knows, the focus, whether observation is paused, the connection, and a way to flag
/// a window sensitive. Requests are sent from the caller's thread and flushed at once, so the
/// bridge thread need not wake.
#[derive(Clone)]
pub struct CompositorLink {
    shared: Arc<LinkState>,
}

struct LinkState {
    windows: RwLock<WindowTable>,
    proxies: Mutex<HashMap<WindowIdent, CrownosContextWindowV1>>,
    connection: Mutex<Option<Connection>>,
    exposure_hook: Mutex<Option<ExposureHook>>,
    focus: watch::Sender<Option<WindowIdent>>,
    paused: AtomicBool,
    locked: AtomicBool,
    /// The scope the compositor granted this engine, as protocol bits; none while unbound.
    granted: AtomicU32,
}

impl std::fmt::Debug for CompositorLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompositorLink")
            .field("connected", &self.connection().is_some())
            .field("observing", &self.is_observing())
            .finish_non_exhaustive()
    }
}

impl Default for CompositorLink {
    fn default() -> Self {
        Self {
            shared: Arc::new(LinkState {
                windows: RwLock::default(),
                proxies: Mutex::default(),
                connection: Mutex::default(),
                exposure_hook: Mutex::default(),
                focus: watch::Sender::new(None),
                paused: AtomicBool::new(false),
                locked: AtomicBool::new(false),
                granted: AtomicU32::new(0),
            }),
        }
    }
}

impl CompositorLink {
    pub fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R {
        match self.shared.windows.read() {
            Ok(windows) => read(&windows),
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    /// Changes with every compositor focus event.
    pub fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>> {
        self.shared.focus.subscribe()
    }

    /// Neither paused by the user nor behind the lock screen.
    pub fn is_observing(&self) -> bool {
        !self.shared.paused.load(Ordering::Relaxed) && !self.shared.locked.load(Ordering::Relaxed)
    }

    /// The compositor grants this engine window titles and text.
    pub fn grants_text(&self) -> bool {
        self.granted().intersects(Scope::Text | Scope::Engine)
    }

    /// The compositor grants this engine captures of window pixels.
    pub fn grants_capture(&self) -> bool {
        self.granted().intersects(Scope::Raw | Scope::Engine)
    }

    fn granted(&self) -> Scope {
        Scope::from_bits_truncate(self.shared.granted.load(Ordering::Relaxed))
    }

    /// A link with the engine scope, as a bound engine has it.
    #[cfg(feature = "test-support")]
    pub fn with_engine_scope() -> Self {
        let link = Self::default();
        link.set_granted(Scope::Engine.bits());
        link
    }

    pub(super) fn set_granted(&self, scope: u32) {
        self.shared.granted.store(scope, Ordering::Relaxed);
    }

    /// The compositor connection while the bridge is bound.
    pub fn connection(&self) -> Option<Connection> {
        self.shared.connection.lock().ok()?.clone()
    }

    /// Asks the compositor to fail captures of `window` for `duration`. Returns whether the
    /// request went out.
    pub fn flag_sensitive(
        &self,
        window: &WindowIdent,
        reason: SensitiveReason,
        duration: Duration,
    ) -> bool {
        let proxy = self
            .shared
            .proxies
            .lock()
            .ok()
            .and_then(|proxies| proxies.get(window).cloned());
        let (Some(proxy), Some(connection)) = (proxy, self.connection()) else {
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
        if let Ok(mut slot) = self.shared.exposure_hook.lock() {
            *slot = Some(hook);
        }
    }

    pub(super) fn exposure_hook(&self) -> Option<ExposureHook> {
        self.shared.exposure_hook.lock().ok()?.clone()
    }

    pub(super) fn update_windows<R>(&self, change: impl FnOnce(&mut WindowTable) -> R) -> R {
        match self.shared.windows.write() {
            Ok(mut windows) => change(&mut windows),
            Err(poisoned) => change(&mut poisoned.into_inner()),
        }
    }

    pub(super) fn set_focus(&self, window: Option<WindowIdent>) {
        self.update_windows(|windows| windows.set_focus(window.clone()));
        self.shared.focus.send_replace(window);
    }

    pub(super) fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub(super) fn set_locked(&self, locked: bool) {
        self.shared.locked.store(locked, Ordering::Relaxed);
    }

    pub(super) fn attach(&self, connection: Connection) {
        if let Ok(mut slot) = self.shared.connection.lock() {
            *slot = Some(connection);
        }
    }

    /// The compositor went away: every window it described is gone with it.
    pub(super) fn detach(&self) {
        if let Ok(mut slot) = self.shared.connection.lock() {
            *slot = None;
        }
        self.set_granted(0);
        if let Ok(mut proxies) = self.shared.proxies.lock() {
            proxies.clear();
        }
        self.update_windows(|windows| *windows = WindowTable::default());
        self.shared.focus.send_replace(None);
    }

    pub(super) fn remember_proxy(&self, window: WindowIdent, proxy: &CrownosContextWindowV1) {
        if let Ok(mut proxies) = self.shared.proxies.lock() {
            proxies.entry(window).or_insert_with(|| proxy.clone());
        }
    }

    pub(super) fn forget_proxy(&self, window: &WindowIdent) {
        if let Ok(mut proxies) = self.shared.proxies.lock() {
            proxies.remove(window);
        }
    }
}
