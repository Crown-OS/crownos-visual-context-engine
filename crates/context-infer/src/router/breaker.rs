use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakerSettings {
    /// How long a failed (runtime, device) rests before one trial call.
    pub cooldown: Duration,
    /// Each failed trial doubles the rest, up to this.
    pub max_cooldown: Duration,
}

impl Default for BreakerSettings {
    fn default() -> Self {
        Self {
            cooldown: Duration::from_secs(30),
            max_cooldown: Duration::from_secs(600),
        }
    }
}

/// What one call says about the health of its (runtime, device).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Healthy,
    /// A backend error, a panic or a hung call.
    Failed,
    /// Busy, a timeout that the backend or a load caused, or an unavailable model.
    Inconclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BreakerState {
    Closed,
    Open {
        until: Instant,
    },
    /// One trial call is running.
    HalfOpen,
}

/// Stops calls to a (runtime, device) after an error or a watchdog timeout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CircuitBreaker {
    state: BreakerState,
    cooldown: Duration,
}

impl CircuitBreaker {
    pub(crate) const fn new(settings: BreakerSettings) -> Self {
        Self {
            state: BreakerState::Closed,
            cooldown: settings.cooldown,
        }
    }

    /// Whether a call may go now, or how long to wait. After the cooldown, it lets one trial
    /// call through.
    pub(crate) fn try_acquire(&mut self, now: Instant) -> Result<(), Duration> {
        match self.state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open { until } if now >= until => {
                self.state = BreakerState::HalfOpen;
                Ok(())
            }
            BreakerState::Open { until } => Err(until - now),
            BreakerState::HalfOpen => Err(self.cooldown),
        }
    }

    pub(crate) fn record(&mut self, verdict: Verdict, now: Instant, settings: BreakerSettings) {
        match (verdict, self.state) {
            (Verdict::Healthy, _) => {
                self.state = BreakerState::Closed;
                self.cooldown = settings.cooldown;
            }
            (Verdict::Failed, state) => {
                if state == BreakerState::HalfOpen {
                    self.cooldown = self.cooldown.saturating_mul(2).min(settings.max_cooldown);
                }
                self.state = BreakerState::Open {
                    until: now + self.cooldown,
                };
            }
            (Verdict::Inconclusive, BreakerState::HalfOpen) => {
                self.state = BreakerState::Open {
                    until: now + self.cooldown,
                };
            }
            (Verdict::Inconclusive, _) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: BreakerSettings = BreakerSettings {
        cooldown: Duration::from_secs(10),
        max_cooldown: Duration::from_secs(25),
    };

    #[test]
    fn opens_on_failure_and_lets_one_trial_through_after_the_cooldown() {
        let start = Instant::now();
        let mut breaker = CircuitBreaker::new(SETTINGS);
        assert!(breaker.try_acquire(start).is_ok());
        breaker.record(Verdict::Failed, start, SETTINGS);
        assert!(breaker.try_acquire(start + Duration::from_secs(9)).is_err());
        assert!(breaker.try_acquire(start + Duration::from_secs(10)).is_ok());
        assert!(
            breaker
                .try_acquire(start + Duration::from_secs(10))
                .is_err()
        );
        breaker.record(Verdict::Healthy, start, SETTINGS);
        assert!(breaker.try_acquire(start + Duration::from_secs(10)).is_ok());
    }

    #[test]
    fn a_failed_trial_doubles_the_cooldown_up_to_the_limit() {
        let start = Instant::now();
        let mut breaker = CircuitBreaker::new(SETTINGS);
        breaker.record(Verdict::Failed, start, SETTINGS);
        let trial = start + Duration::from_secs(10);
        assert!(breaker.try_acquire(trial).is_ok());
        breaker.record(Verdict::Failed, trial, SETTINGS);
        assert!(
            breaker
                .try_acquire(trial + Duration::from_secs(19))
                .is_err()
        );
        let second_trial = trial + Duration::from_secs(20);
        assert!(breaker.try_acquire(second_trial).is_ok());
        breaker.record(Verdict::Failed, second_trial, SETTINGS);
        assert!(
            breaker
                .try_acquire(second_trial + Duration::from_secs(24))
                .is_err()
        );
        assert!(
            breaker
                .try_acquire(second_trial + Duration::from_secs(25))
                .is_ok()
        );
    }

    #[test]
    fn an_inconclusive_trial_reopens_with_the_same_cooldown() {
        let start = Instant::now();
        let mut breaker = CircuitBreaker::new(SETTINGS);
        breaker.record(Verdict::Inconclusive, start, SETTINGS);
        assert!(breaker.try_acquire(start).is_ok());
        breaker.record(Verdict::Failed, start, SETTINGS);
        let trial = start + Duration::from_secs(10);
        assert!(breaker.try_acquire(trial).is_ok());
        breaker.record(Verdict::Inconclusive, trial, SETTINGS);
        assert!(breaker.try_acquire(trial + Duration::from_secs(9)).is_err());
        assert!(breaker.try_acquire(trial + Duration::from_secs(10)).is_ok());
    }
}
