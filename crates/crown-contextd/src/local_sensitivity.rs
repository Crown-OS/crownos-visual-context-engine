//! Windows the engine itself treats as sensitive for a while, because a secret was seen in
//! them. This holds even when the compositor did not take the flag.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use context_core::WindowIdent;

#[derive(Debug, Default)]
pub(crate) struct LocalSensitivity {
    until: HashMap<WindowIdent, Instant>,
}

impl LocalSensitivity {
    pub(crate) fn mark(&mut self, window: &WindowIdent, duration: Duration) {
        self.until.insert(window.clone(), Instant::now() + duration);
    }

    pub(crate) fn is_marked(&self, window: &WindowIdent) -> bool {
        self.until
            .get(window)
            .is_some_and(|until| Instant::now() < *until)
    }

    pub(crate) fn forget_expired(&mut self) {
        let now = Instant::now();
        self.until.retain(|_, until| now < *until);
    }
}
