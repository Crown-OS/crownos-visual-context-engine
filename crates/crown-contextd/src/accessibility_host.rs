//! What the accessibility collector may see: the compositor's windows, under the same policy and
//! pause state as the pipeline, checked before a read rather than after it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use context_collect::WindowTable;
use context_collect::a11y::AccessibilityHost;
use context_collect::{CompositorLink, SensitiveReason};
use context_core::{WindowIdent, WindowRecord};
use context_policy::Policy;
use tokio::sync::watch;

use crate::shared::Shared;
use crate::text_gate::may_read_window;

/// How long the compositor refuses captures of a window that shows a password field. The
/// collector flags the window again on each read while the field stays.
const PASSWORD_FIELD_FLAG_DURATION: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(crate) struct ContextHost {
    link: CompositorLink,
    policy: Arc<Policy>,
    shared: Arc<Mutex<Shared>>,
}

impl ContextHost {
    pub(crate) const fn new(
        link: CompositorLink,
        policy: Arc<Policy>,
        shared: Arc<Mutex<Shared>>,
    ) -> Self {
        Self {
            link,
            policy,
            shared,
        }
    }
}

impl AccessibilityHost for ContextHost {
    fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R {
        self.link.with_windows(read)
    }

    fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>> {
        self.link.watch_focus()
    }

    /// The content gate, and the frame title the toolkit shows, which can differ from the
    /// compositor's.
    fn may_read(&self, window: &WindowRecord, frame_title: &str) -> bool {
        !self.policy.is_private_title(frame_title)
            && self
                .shared
                .lock()
                .is_ok_and(|shared| may_read_window(&shared, &self.link, &self.policy, window))
    }

    /// Flags the window at the compositor, and holds it in the engine too, so a capture that is
    /// already due does not read it.
    fn flag_sensitive(&self, window: &WindowIdent) {
        self.link.flag_sensitive(
            window,
            SensitiveReason::SecretText,
            PASSWORD_FIELD_FLAG_DURATION,
        );
        if let Ok(mut shared) = self.shared.lock() {
            shared
                .locally_sensitive
                .mark(window, PASSWORD_FIELD_FLAG_DURATION);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use context_core::Sensitivity;
    use context_policy::VisualContext;

    use super::*;

    fn host() -> ContextHost {
        let section = VisualContext {
            enabled: true,
            ..VisualContext::default()
        };
        ContextHost::new(
            CompositorLink::with_engine_scope(),
            Arc::new(Policy::from_section(&section).unwrap()),
            Arc::new(Mutex::new(Shared::new(true))),
        )
    }

    fn window(app_id: &str) -> WindowRecord {
        WindowRecord {
            ident: WindowIdent("w".into()),
            app_id: app_id.into(),
            title: "notes.txt".into(),
            ..WindowRecord::default()
        }
    }

    #[test]
    fn an_ordinary_window_may_be_read() {
        assert!(host().may_read(&window("org.gnome.TextEditor"), "notes.txt"));
    }

    #[test]
    fn excluded_apps_private_frames_and_sensitive_windows_are_not_read() {
        let host = host();
        assert!(!host.may_read(&window("org.keepassxc.KeePassXC"), "Passwords"));
        assert!(!host.may_read(&window("org.mozilla.firefox"), "Bank — Private Browsing"));
        let mut sensitive = window("org.gnome.TextEditor");
        sensitive.sensitivity = Sensitivity {
            password_input: true,
            ..Sensitivity::default()
        };
        assert!(!host.may_read(&sensitive, "notes.txt"));
    }

    #[test]
    fn nothing_is_read_while_the_engine_is_paused() {
        let host = host();
        host.shared.lock().unwrap().paused = true;
        assert!(!host.may_read(&window("org.gnome.TextEditor"), "notes.txt"));
    }
}
