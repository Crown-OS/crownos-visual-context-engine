use context_core::{Micros, Timestamp};

/// `HH:MM:SS` in UTC.
pub(crate) fn clock(t: Timestamp) -> String {
    let seconds = t.0.div_euclid(1_000_000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds % 3_600 / 60,
        seconds % 60
    )
}

pub(crate) fn duration_text(duration: Micros) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3_600 {
        format!("{}h{:02}m", seconds / 3_600, seconds % 3_600 / 60)
    } else if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_and_durations_read_well() {
        assert_eq!(clock(Timestamp::from_secs(86_400 + 3_661)), "01:01:01");
        assert_eq!(duration_text(Micros::from_secs(59)), "59s");
        assert_eq!(duration_text(Micros::from_secs(125)), "2m05s");
        assert_eq!(duration_text(Micros::from_secs(3_660)), "1h01m");
    }
}
