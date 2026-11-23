use std::time::Duration;

use tokio::time::Instant;

/// A read cheaper than the first bound is tier 0, cheaper than the second tier 1, and so on.
const COST_BOUNDS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(150),
    Duration::from_millis(250),
];
/// The least time between two reads of one application, per tier.
const INTERVALS: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(4),
    Duration::from_secs(15),
];
const SLOWEST_TIER: usize = INTERVALS.len() - 1;

/// How often one application may be read: an expensive or cut-short read moves it straight to a
/// slower tier, and each cheap read moves it back by one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AppBackoff {
    tier: usize,
    ready_at: Instant,
}

impl AppBackoff {
    pub(crate) const fn new(now: Instant) -> Self {
        Self {
            tier: 0,
            ready_at: now,
        }
    }

    pub(crate) const fn ready_at(&self) -> Instant {
        self.ready_at
    }

    pub(crate) fn record(&mut self, cost: Duration, cut_short: bool, now: Instant) {
        let cost_tier = if cut_short {
            SLOWEST_TIER
        } else {
            COST_BOUNDS
                .iter()
                .position(|bound| cost < *bound)
                .unwrap_or(SLOWEST_TIER)
        };
        self.tier = cost_tier.max(self.tier.saturating_sub(1));
        let interval = INTERVALS.get(self.tier).copied().unwrap_or(Duration::MAX);
        self.ready_at = now + interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn millis(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn cheap_reads_stay_fast() {
        let now = Instant::now();
        let mut backoff = AppBackoff::new(now);
        assert_eq!(backoff.ready_at(), now);
        backoff.record(millis(20), false, now);
        assert_eq!(backoff.ready_at(), now + millis(250));
    }

    #[test]
    fn cost_picks_the_tier_and_recovery_is_one_tier_at_a_time() {
        let now = Instant::now();
        let mut backoff = AppBackoff::new(now);
        backoff.record(millis(120), false, now);
        assert_eq!(backoff.ready_at(), now + Duration::from_secs(1));
        backoff.record(millis(200), false, now);
        assert_eq!(backoff.ready_at(), now + Duration::from_secs(4));
        backoff.record(millis(10), false, now);
        assert_eq!(backoff.ready_at(), now + Duration::from_secs(1));
        backoff.record(millis(10), false, now);
        assert_eq!(backoff.ready_at(), now + millis(250));
    }

    #[test]
    fn a_read_cut_short_goes_to_the_slowest_tier() {
        let now = Instant::now();
        let mut backoff = AppBackoff::new(now);
        backoff.record(millis(5), true, now);
        assert_eq!(backoff.ready_at(), now + Duration::from_secs(15));
        backoff.record(millis(400), false, now);
        assert_eq!(backoff.ready_at(), now + Duration::from_secs(15));
    }
}
