//! The one gate in front of every read of window content, by capture or by the accessibility
//! collector. The checks run in this order: enabled, paused, locked, access, policy, local
//! sensitivity.

use context_collect::CompositorLink;
use context_core::WindowRecord;
use context_policy::Policy;

use crate::shared::Shared;

/// Whether the content of `window` may be read now.
pub(crate) fn may_read_window(
    shared: &Shared,
    link: &CompositorLink,
    policy: &Policy,
    window: &WindowRecord,
) -> bool {
    shared.enabled
        && !shared.paused
        && link.is_observing()
        && shared.engine.is_observing()
        && link.grants_text()
        && policy.judge(window).allows_text()
        && !shared.locally_sensitive.is_marked(&window.ident)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use context_core::{Sensitivity, WindowIdent};
    use context_policy::VisualContext;

    use super::*;

    fn policy() -> Policy {
        Policy::from_section(&VisualContext {
            enabled: true,
            ..VisualContext::default()
        })
        .unwrap()
    }

    fn window() -> WindowRecord {
        WindowRecord {
            ident: WindowIdent("w".into()),
            app_id: "org.gnome.TextEditor".into(),
            title: "notes.txt".into(),
            ..WindowRecord::default()
        }
    }

    #[test]
    fn each_hold_closes_the_gate() {
        let link = CompositorLink::with_engine_scope();
        let policy = policy();
        let mut shared = Shared::new(true);
        assert!(may_read_window(&shared, &link, &policy, &window()));

        assert!(!may_read_window(
            &shared,
            &CompositorLink::default(),
            &policy,
            &window()
        ));

        shared.paused = true;
        assert!(!may_read_window(&shared, &link, &policy, &window()));
        shared.paused = false;

        let mut sensitive = window();
        sensitive.sensitivity = Sensitivity {
            password_input: true,
            ..Sensitivity::default()
        };
        assert!(!may_read_window(&shared, &link, &policy, &sensitive));

        shared
            .locally_sensitive
            .mark(&window().ident, Duration::from_secs(60));
        assert!(!may_read_window(&shared, &link, &policy, &window()));

        assert!(!may_read_window(
            &Shared::new(false),
            &link,
            &policy,
            &window()
        ));
    }
}
