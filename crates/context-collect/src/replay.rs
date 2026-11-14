use std::sync::mpsc::SyncSender;
use std::time::Duration;

use context_core::Observation;

/// Feeds a recorded or synthetic trace into `outlet` as if the compositor sent it, `speed`
/// times faster than it happened; zero or less plays it back as fast as the receiver takes it.
pub fn replay(observations: Vec<Observation>, outlet: &SyncSender<Observation>, speed: f64) {
    let mut previous = None;
    for observation in observations {
        if speed > 0.0
            && let Some(before) = previous
        {
            let gap = observation.t.since(before).0 as f64 / speed;
            if gap > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(gap / 1_000_000.0));
            }
        }
        previous = Some(observation.t);
        if outlet.send(observation).is_err() {
            return;
        }
    }
}
