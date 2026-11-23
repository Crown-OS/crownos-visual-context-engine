//! Lets jobs run, or holds them back before the system sleeps. A job owns its memfds and their
//! read-only mappings, so they are unmapped and closed when the job ends; the gate counts the
//! running jobs so the sleep can wait for that.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Debug, Default)]
struct GateState {
    closed: bool,
    running: usize,
}

#[derive(Debug, Default)]
pub(crate) struct JobGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

/// One running job. Dropping it ends the job for the gate.
#[derive(Debug)]
pub(crate) struct RunningJob<'a>(&'a JobGate);

impl Drop for RunningJob<'_> {
    fn drop(&mut self) {
        if let Some(mut state) = self.0.lock() {
            state.running = state.running.saturating_sub(1);
            self.0.changed.notify_all();
        }
    }
}

impl JobGate {
    /// A place for one job, or `None` while the gate is closed.
    pub(crate) fn enter(&self) -> Option<RunningJob<'_>> {
        let mut state = self.lock()?;
        if state.closed {
            return None;
        }
        state.running += 1;
        Some(RunningJob(self))
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.lock().is_none_or(|state| state.closed)
    }

    /// Refuses new jobs, then waits up to `timeout` for the running ones to end. Returns
    /// whether none is left.
    pub(crate) fn close_and_wait(&self, timeout: Duration) -> bool {
        let Some(mut state) = self.lock() else {
            return false;
        };
        state.closed = true;
        self.changed
            .wait_timeout_while(state, timeout, |state| state.running > 0)
            .is_ok_and(|(state, _)| state.running == 0)
    }

    pub(crate) fn open(&self) {
        if let Some(mut state) = self.lock() {
            state.closed = false;
        }
    }

    fn lock(&self) -> Option<MutexGuard<'_, GateState>> {
        self.state.lock().ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_gate_refuses_jobs_and_waits_for_the_running_ones() {
        let gate = JobGate::default();
        let job = gate.enter().unwrap();
        assert!(!gate.close_and_wait(Duration::from_millis(10)));
        assert!(gate.enter().is_none());
        drop(job);
        assert!(gate.close_and_wait(Duration::ZERO));
        gate.open();
        assert!(gate.enter().is_some());
    }
}
