//! What the pipeline thread and the API share.

use context_engine::Engine;
use context_protocol::CaptureState;

use crate::local_sensitivity::LocalSensitivity;

#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) engine: Engine,
    pub(crate) enabled: bool,
    pub(crate) paused: bool,
    pub(crate) pause_reason: Option<String>,
    pub(crate) observations_seen: u64,
    pub(crate) observations_dropped: u64,
    /// Windows the engine itself holds as sensitive, for capture and the a11y collector alike.
    pub(crate) locally_sensitive: LocalSensitivity,
}

impl Shared {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            engine: Engine::default(),
            enabled,
            paused: false,
            pause_reason: None,
            observations_seen: 0,
            observations_dropped: 0,
            locally_sensitive: LocalSensitivity::default(),
        }
    }

    pub(crate) const fn capture_state(&self) -> CaptureState {
        if self.paused || !self.enabled {
            CaptureState::Paused
        } else if self.engine.is_observing() {
            CaptureState::Observing
        } else {
            CaptureState::Degraded
        }
    }
}
