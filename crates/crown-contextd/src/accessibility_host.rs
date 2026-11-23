//! What the accessibility collector may see: the compositor's windows, under the same policy and
//! pause state as the pipeline, checked before a read rather than after it.

use std::sync::{Arc, Mutex};

use context_collect::CompositorLink;
use context_collect::WindowTable;
use context_collect::a11y::{AccessibilityHost, compositor_allows_read};
use context_core::{WindowIdent, WindowRecord};
use context_policy::Policy;
use tokio::sync::watch;

use crate::shared::Shared;

#[derive(Debug)]
pub(crate) struct ContextHost {
    link: CompositorLink,
    policy: Policy,
    shared: Arc<Mutex<Shared>>,
}

impl ContextHost {
    pub(crate) const fn new(
        link: CompositorLink,
        policy: Policy,
        shared: Arc<Mutex<Shared>>,
    ) -> Self {
        Self {
            link,
            policy,
            shared,
        }
    }

    fn engine_observes(&self) -> bool {
        self.shared
            .lock()
            .is_ok_and(|shared| shared.enabled && !shared.paused)
    }
}

impl AccessibilityHost for ContextHost {
    fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R {
        self.link.with_windows(read)
    }

    fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>> {
        self.link.watch_focus()
    }

    fn may_read(&self, window: &WindowRecord) -> bool {
        compositor_allows_read(&self.link, window)
            && self.policy.judge(window).allows_text()
            && self.engine_observes()
    }

    fn flag_sensitive(&self, window: &WindowIdent) {
        self.link.flag_sensitive(window);
    }
}
