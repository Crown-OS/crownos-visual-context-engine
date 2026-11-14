use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Microseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(pub i64);

/// A span of time in microseconds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Micros(pub i64);

impl Micros {
    pub const ZERO: Self = Self(0);

    pub const fn from_secs(seconds: i64) -> Self {
        Self(seconds * 1_000_000)
    }

    pub const fn from_millis(milliseconds: i64) -> Self {
        Self(milliseconds * 1_000)
    }

    pub const fn as_secs(self) -> i64 {
        self.0 / 1_000_000
    }

    pub const fn as_millis(self) -> i64 {
        self.0 / 1_000
    }

    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl Timestamp {
    pub fn now() -> Self {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self(i64::try_from(since_epoch.as_micros()).unwrap_or(i64::MAX))
    }

    pub const fn from_secs(seconds: i64) -> Self {
        Self(seconds * 1_000_000)
    }

    pub const fn plus(self, duration: Micros) -> Self {
        Self(self.0.saturating_add(duration.0))
    }

    /// `self - earlier`, zero when `earlier` is later.
    pub const fn since(self, earlier: Self) -> Micros {
        let difference = self.0.saturating_sub(earlier.0);
        Micros(if difference < 0 { 0 } else { difference })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: Timestamp,
    pub end: Timestamp,
}

impl TimeRange {
    pub const fn new(start: Timestamp, end: Timestamp) -> Self {
        Self { start, end }
    }

    pub const fn duration(&self) -> Micros {
        self.end.since(self.start)
    }

    pub const fn contains(&self, t: Timestamp) -> bool {
        self.start.0 <= t.0 && t.0 < self.end.0
    }

    pub const fn overlaps(&self, other: &Self) -> bool {
        self.start.0 < other.end.0 && other.start.0 < self.end.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_half_open() {
        let range = TimeRange::new(Timestamp(10), Timestamp(20));
        assert!(range.contains(Timestamp(10)));
        assert!(!range.contains(Timestamp(20)));
        assert_eq!(range.duration(), Micros(10));
        assert!(range.overlaps(&TimeRange::new(Timestamp(19), Timestamp(30))));
        assert!(!range.overlaps(&TimeRange::new(Timestamp(20), Timestamp(30))));
    }

    #[test]
    fn since_never_goes_negative() {
        assert_eq!(Timestamp(5).since(Timestamp(9)), Micros::ZERO);
        assert_eq!(Timestamp(9).since(Timestamp(5)), Micros(4));
    }
}
