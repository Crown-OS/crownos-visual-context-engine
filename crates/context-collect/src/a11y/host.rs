use context_core::{WindowIdent, WindowRecord};
use tokio::sync::watch;

use crate::{CompositorLink, WindowTable};

/// What the collector needs from the rest of the engine: the windows, the focus, the policy and
/// a way to flag a window sensitive.
pub trait AccessibilityHost: Send + 'static {
    fn with_windows<R>(&self, read: impl FnOnce(&WindowTable) -> R) -> R;

    fn watch_focus(&self) -> watch::Receiver<Option<WindowIdent>>;

    /// Whether the text of `window` may be read at all. Asked before every read, so an excluded,
    /// sensitive or paused window is never read.
    fn may_read(&self, window: &WindowRecord) -> bool;

    fn flag_sensitive(&self, window: &WindowIdent);
}

/// The compositor's own verdict: no read while paused or locked, of a sensitive window or of
/// moving pictures. The engine's policy comes on top of this.
pub fn compositor_allows_read(link: &CompositorLink, window: &WindowRecord) -> bool {
    link.is_observing() && !window.sensitivity.any() && !window.content_type.is_motion()
}
