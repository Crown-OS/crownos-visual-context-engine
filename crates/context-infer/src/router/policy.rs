use crownos_config::system::ai::Device;

use crate::registry::{Candidate, Tier};

/// What the user and the power state allow right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub on_battery: bool,
    /// The user agreed to send this kind of work to a remote server.
    pub remote_consent: bool,
    pub tier: Tier,
}

impl Policy {
    pub(crate) fn permits(self, candidate: &Candidate) -> bool {
        (candidate.device != Device::Remote || self.remote_consent)
            && (candidate.on_battery || !self.on_battery)
            && candidate.min_tier <= self.tier
    }
}
