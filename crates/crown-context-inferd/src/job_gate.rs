//! Lets jobs run, ends them early, or holds them back before the system sleeps. A job owns its
//! memfds and their read-only mappings, so they are unmapped and closed when the job ends.
//!
//! A cancelled job that waits for its model does not run, and its caller stops waiting at once.
//! A model call that already runs cannot be stopped from outside: its mapping stays until the
//! call returns, within the job's deadline and hang budget.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use context_infer::JobCancel;
use crownos_ipc::PeerId;

#[derive(Debug)]
struct Entry<P> {
    peer: P,
    job_id: Option<u64>,
    cancel: JobCancel,
}

#[derive(Debug)]
struct GateState<P> {
    closed: bool,
    next_key: u64,
    running: HashMap<u64, Entry<P>>,
}

/// The gate of the jobs of the peers `P` of the inference socket.
#[derive(Debug)]
pub(crate) struct JobGate<P = PeerId> {
    state: Mutex<GateState<P>>,
    changed: Condvar,
}

impl<P> Default for JobGate<P> {
    fn default() -> Self {
        Self {
            state: Mutex::new(GateState {
                closed: false,
                next_key: 0,
                running: HashMap::new(),
            }),
            changed: Condvar::new(),
        }
    }
}

/// One admitted job. Dropping it ends the job for the gate.
#[derive(Debug)]
pub(crate) struct RunningJob<P = PeerId> {
    gate: Arc<JobGate<P>>,
    key: u64,
    cancel: JobCancel,
}

impl<P> RunningJob<P> {
    pub(crate) fn cancel_token(&self) -> JobCancel {
        self.cancel.clone()
    }
}

impl<P> Drop for RunningJob<P> {
    fn drop(&mut self) {
        if let Some(mut state) = self.gate.lock() {
            state.running.remove(&self.key);
            self.gate.changed.notify_all();
        }
    }
}

impl<P> JobGate<P> {
    fn lock(&self) -> Option<MutexGuard<'_, GateState<P>>> {
        self.state.lock().ok()
    }
}

impl<P: Copy + PartialEq> JobGate<P> {
    /// A place for one job of `peer`, or `None` while the gate is closed.
    pub(crate) fn enter(self: &Arc<Self>, peer: P, job_id: Option<u64>) -> Option<RunningJob<P>> {
        let mut state = self.lock()?;
        if state.closed {
            return None;
        }
        let key = state.next_key;
        state.next_key += 1;
        let cancel = JobCancel::default();
        state.running.insert(
            key,
            Entry {
                peer,
                job_id,
                cancel: cancel.clone(),
            },
        );
        Some(RunningJob {
            gate: Arc::clone(self),
            key,
            cancel,
        })
    }

    /// Cancels the job `job_id` of `peer`. Returns whether it runs.
    pub(crate) fn cancel(&self, peer: P, job_id: u64) -> bool {
        self.cancel_where(|entry| entry.peer == peer && entry.job_id == Some(job_id)) > 0
    }

    /// Cancels the jobs of peers that are gone: their connection closed.
    pub(crate) fn cancel_jobs_of_gone_peers(&self, live: &[P]) -> usize {
        self.cancel_where(|entry| !live.contains(&entry.peer))
    }

    fn cancel_where(&self, matches: impl Fn(&Entry<P>) -> bool) -> usize {
        let Some(state) = self.lock() else {
            return 0;
        };
        state
            .running
            .values()
            .filter(|entry| matches(entry) && !entry.cancel.is_cancelled())
            .inspect(|entry| entry.cancel.cancel())
            .count()
    }

    /// Refuses new jobs and cancels the running ones, then waits up to `timeout` for the model
    /// calls that already run. Returns whether no job is left.
    pub(crate) fn close_and_cancel(&self, timeout: Duration) -> bool {
        let Some(mut state) = self.lock() else {
            return false;
        };
        state.closed = true;
        for entry in state.running.values() {
            entry.cancel.cancel();
        }
        self.changed
            .wait_timeout_while(state, timeout, |state| !state.running.is_empty())
            .is_ok_and(|(state, _)| state.running.is_empty())
    }

    pub(crate) fn open(&self) {
        if let Some(mut state) = self.lock() {
            state.closed = false;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_gate_refuses_jobs_and_cancels_the_running_ones() {
        let gate = Arc::new(JobGate::<u32>::default());
        let peer = 1;
        let job = gate.enter(peer, Some(1)).unwrap();
        assert!(!gate.close_and_cancel(Duration::from_millis(10)));
        assert!(job.cancel_token().is_cancelled());
        assert!(gate.enter(peer, Some(2)).is_none());
        drop(job);
        assert!(gate.close_and_cancel(Duration::ZERO));
        gate.open();
        assert!(gate.enter(peer, Some(3)).is_some());
    }

    #[test]
    fn sleep_waits_for_the_model_call_that_holds_the_job() {
        let gate = Arc::new(JobGate::<u32>::default());
        let lease = context_infer::JobLease::holding(gate.enter(1, Some(1)).unwrap());
        let model_call = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(lease);
        });
        let started = std::time::Instant::now();
        assert!(gate.close_and_cancel(Duration::from_secs(4)));
        assert!(started.elapsed() >= Duration::from_millis(150));
        model_call.join().unwrap();
    }

    #[test]
    fn a_job_ends_by_its_id_or_with_its_connection() {
        let gate = Arc::new(JobGate::<u32>::default());
        let (first, second) = (1, 2);
        let one = gate.enter(first, Some(7)).unwrap();
        let two = gate.enter(second, Some(7)).unwrap();
        assert!(!gate.cancel(first, 8));
        assert!(gate.cancel(first, 7));
        assert!(one.cancel_token().is_cancelled());
        assert!(!two.cancel_token().is_cancelled());
        assert_eq!(gate.cancel_jobs_of_gone_peers(&[first]), 1);
        assert!(two.cancel_token().is_cancelled());
    }
}
