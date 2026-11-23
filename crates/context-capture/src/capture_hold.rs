use std::sync::atomic::{AtomicU8, Ordering};

use crate::cancellation::Cancellation;

/// Why captures are held. Each reason is released on its own, so waking from sleep does not
/// end a pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureHold {
    Sleep,
    Paused,
    Locked,
    /// The process is briefly readable through `/proc` while the bridge connects.
    IdentityExposed,
}

impl CaptureHold {
    const fn bit(self) -> u8 {
        match self {
            Self::Sleep => 1,
            Self::Paused => 2,
            Self::Locked => 4,
            Self::IdentityExposed => 8,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct CaptureHolds(AtomicU8);

impl Cancellation for CaptureHolds {
    fn is_cancelled(&self) -> bool {
        self.is_held()
    }
}

impl CaptureHolds {
    pub(crate) fn hold(&self, reason: CaptureHold) {
        self.0.fetch_or(reason.bit(), Ordering::SeqCst);
    }

    /// Returns whether no reason is left.
    pub(crate) fn release(&self, reason: CaptureHold) -> bool {
        self.0.fetch_and(!reason.bit(), Ordering::SeqCst) & !reason.bit() == 0
    }

    pub(crate) fn is_held(&self) -> bool {
        self.0.load(Ordering::SeqCst) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_reason_is_released_on_its_own() {
        let holds = CaptureHolds::default();
        holds.hold(CaptureHold::Paused);
        holds.hold(CaptureHold::Sleep);
        assert!(!holds.release(CaptureHold::Sleep));
        assert!(holds.is_held());
        assert!(holds.release(CaptureHold::Paused));
        assert!(!holds.is_held());
    }
}
