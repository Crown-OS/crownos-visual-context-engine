use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const JOIN_CHECK: Duration = Duration::from_millis(10);

/// Joins `thread` when it ends within `timeout`. A thread that is still running then is left
/// to end on its own, so a stop never hangs; the caller says which thread it was. Returns
/// whether the thread ended.
pub fn join_within(thread: JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(JOIN_CHECK);
    }
    let _ = thread.join();
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_that_ends_is_joined_and_one_that_hangs_is_left() {
        let quick = std::thread::spawn(|| {});
        assert!(join_within(quick, Duration::from_secs(1)));
        let (_keep, hang) = std::sync::mpsc::channel::<()>();
        let stuck = std::thread::spawn(move || {
            let _ = hang.recv();
        });
        assert!(!join_within(stuck, Duration::from_millis(30)));
    }
}
