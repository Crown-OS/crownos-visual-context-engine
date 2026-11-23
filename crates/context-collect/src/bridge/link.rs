use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use context_core::WindowIdent;
use crownos_protocols::context::v1::client::crownos_context_window_v1::{
    CrownosContextWindowV1, FlagReason,
};
use tokio::sync::watch;
use wayland_client::Connection;

use crate::window_table::WindowTable;

/// How long a window stays sensitive after the engine saw a secret or a password field in it.
pub const SENSITIVE_FOR: Duration = Duration::from_secs(60);

/// What the bridge thread shares with the other collectors: the windows it knows, the focus,
/// whether observation is paused, and a way to flag a window sensitive.
#[derive(Debug, Clone)]
pub struct CompositorLink {
    shared: Arc<LinkState>,
}

#[derive(Debug)]
struct LinkState {
    windows: RwLock<WindowTable>,
    proxies: Mutex<HashMap<WindowIdent, CrownosContextWindowV1>>,
    connection: Mutex<Option<Connection>>,
    focus: watch::Sender<Option<WindowIdent>>,
    paused: AtomicBool,
    locked: AtomicBool,
}

impl Default for CompositorLink {
    fn default() -> Self {
        Self {
            shared: Arc::new(LinkState {
                windows: RwLock::default(),
                proxies: Mutex::default(),
                connection: Mutex::default(),
                focus: watch::Sender::new(None),
                paused: AtomicBool::new(false),
                locked: AtomicBool::new(false),
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

    /// Asks the compositor to treat the window as sensitive for [`SENSITIVE_FOR`]. Does nothing
    /// while the bridge is not bound.
    pub fn flag_sensitive(&self, window: &WindowIdent) {
        let proxy = self
            .shared
            .proxies
            .lock()
            .ok()
            .and_then(|proxies| proxies.get(window).cloned());
        let Some(proxy) = proxy else {
            return;
        };
        let duration_ms = u32::try_from(SENSITIVE_FOR.as_millis()).unwrap_or(u32::MAX);
        proxy.flag_sensitive(FlagReason::SecretText, duration_ms);
        if let Ok(connection) = self.shared.connection.lock()
            && let Some(connection) = connection.as_ref()
            && let Err(error) = connection.flush()
        {
            tracing::warn!(%error, "cannot send flag_sensitive to the compositor");
        }
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
