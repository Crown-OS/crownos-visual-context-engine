//! ActivityWatch's trick: a stream of "still here" samples becomes one row per stretch.

use context_core::{Micros, TimeRange, Timestamp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heartbeat<T> {
    pub data: T,
    pub range: TimeRange,
}

/// Extends the last heartbeat when `data` equals it and `t` is within `pulsetime` of its end;
/// otherwise starts a new one. Returns whether a new one started.
pub fn merge_heartbeat<T: PartialEq>(
    beats: &mut Vec<Heartbeat<T>>,
    data: T,
    t: Timestamp,
    pulsetime: Micros,
) -> bool {
    if let Some(last) = beats.last_mut()
        && last.data == data
        && t.since(last.range.end).0 <= pulsetime.0
        && t.0 >= last.range.start.0
    {
        if t.0 > last.range.end.0 {
            last.range.end = t;
        }
        return false;
    }
    beats.push(Heartbeat {
        data,
        range: TimeRange::new(t, t),
    });
    true
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn equal_samples_within_the_pulse_merge() {
        let mut beats = Vec::new();
        let pulse = Micros::from_secs(60);
        assert!(merge_heartbeat(
            &mut beats,
            "a",
            Timestamp::from_secs(0),
            pulse
        ));
        assert!(!merge_heartbeat(
            &mut beats,
            "a",
            Timestamp::from_secs(30),
            pulse
        ));
        assert!(!merge_heartbeat(
            &mut beats,
            "a",
            Timestamp::from_secs(80),
            pulse
        ));
        assert!(merge_heartbeat(
            &mut beats,
            "b",
            Timestamp::from_secs(90),
            pulse
        ));
        assert!(merge_heartbeat(
            &mut beats,
            "b",
            Timestamp::from_secs(200),
            pulse
        ));
        assert_eq!(beats.len(), 3);
        assert_eq!(beats[0].range.duration(), Micros::from_secs(80));
    }
}
