use std::any::Any;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crownos_config::system::ai::{Device, Runtime};

use super::benchmark::BenchmarkCache;
use super::breaker::{BreakerSettings, CircuitBreaker};
use super::policy::Policy;
use super::worker::{JobSender, ModelWorker};
use crate::registry::{Candidate, Capability};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LoadedKey {
    capability: Capability,
    model: String,
    runtime: Runtime,
    device: Device,
}

impl LoadedKey {
    pub(crate) fn new(capability: Capability, candidate: &Candidate) -> Self {
        Self {
            capability,
            model: candidate.model.clone(),
            runtime: candidate.runtime,
            device: candidate.device,
        }
    }
}

/// A [`ModelWorker`] of any capability behind `Any`, so one map holds them all.
struct LoadedWorker {
    worker: Box<dyn Any + Send + Sync>,
    last_used: Instant,
}

/// The router's mutable part. Never held during a call or a model drop.
pub(crate) struct RouterState {
    pub(crate) policy: Policy,
    pub(crate) benchmarks: BenchmarkCache,
    breakers: HashMap<(Runtime, Device), CircuitBreaker>,
    workers: HashMap<LoadedKey, LoadedWorker>,
}

impl RouterState {
    pub(crate) fn new(policy: Policy, benchmarks: BenchmarkCache) -> Self {
        Self {
            policy,
            benchmarks,
            breakers: HashMap::new(),
            workers: HashMap::new(),
        }
    }

    pub(crate) fn breaker(
        &mut self,
        candidate: &Candidate,
        settings: BreakerSettings,
    ) -> &mut CircuitBreaker {
        self.breakers
            .entry((candidate.runtime, candidate.device))
            .or_insert_with(|| CircuitBreaker::new(settings))
    }

    pub(crate) fn sender<C: ?Sized + Send + Sync + 'static>(
        &mut self,
        key: &LoadedKey,
        now: Instant,
    ) -> Option<JobSender<C>> {
        let entry = self.workers.get_mut(key)?;
        entry.last_used = now;
        entry
            .worker
            .downcast_ref::<ModelWorker<C>>()
            .map(ModelWorker::sender)
    }

    pub(crate) fn insert_worker<C: ?Sized + Send + Sync + 'static>(
        &mut self,
        key: LoadedKey,
        worker: ModelWorker<C>,
        now: Instant,
    ) {
        self.workers.insert(
            key,
            LoadedWorker {
                worker: Box::new(worker),
                last_used: now,
            },
        );
    }

    pub(crate) fn touch(&mut self, key: &LoadedKey, now: Instant) {
        if let Some(entry) = self.workers.get_mut(key) {
            entry.last_used = now;
        }
    }

    pub(crate) fn take_worker<C: ?Sized + Send + Sync + 'static>(
        &mut self,
        key: &LoadedKey,
    ) -> Option<ModelWorker<C>> {
        let entry = self.workers.remove(key)?;
        entry
            .worker
            .downcast::<ModelWorker<C>>()
            .ok()
            .map(|worker| *worker)
    }

    pub(crate) fn loaded_models(&self) -> Vec<(Capability, String, Runtime, Device)> {
        self.workers
            .keys()
            .map(|key| (key.capability, key.model.clone(), key.runtime, key.device))
            .collect()
    }

    /// The caller drops what this returns after it releases the lock.
    #[must_use]
    pub(crate) fn take_idle_workers(
        &mut self,
        now: Instant,
        idle_for: Duration,
        resident: &[Capability],
    ) -> Vec<Box<dyn Any + Send + Sync>> {
        let idle: Vec<LoadedKey> = self
            .workers
            .iter()
            .filter(|(key, entry)| {
                !resident.contains(&key.capability)
                    && now.saturating_duration_since(entry.last_used) >= idle_for
            })
            .map(|(key, _)| key.clone())
            .collect();
        idle.iter()
            .filter_map(|key| self.workers.remove(key))
            .map(|entry| entry.worker)
            .collect()
    }
}
