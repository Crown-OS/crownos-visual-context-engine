use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crownos_config::system::ai::{Device, Runtime};

use super::benchmark::BenchmarkCache;
use super::breaker::{BreakerSettings, CircuitBreaker};
use super::policy::Policy;
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

/// An `Arc<dyn Capability>` behind `Any`, so one map holds every capability.
struct LoadedModel {
    model: Arc<dyn Any + Send + Sync>,
    last_used: Instant,
}

/// The router's mutable part. Never held during a call.
pub(crate) struct RouterState {
    pub(crate) policy: Policy,
    pub(crate) benchmarks: BenchmarkCache,
    breakers: HashMap<(Runtime, Device), CircuitBreaker>,
    loaded: HashMap<LoadedKey, LoadedModel>,
}

impl RouterState {
    pub(crate) fn new(policy: Policy, benchmarks: BenchmarkCache) -> Self {
        Self {
            policy,
            benchmarks,
            breakers: HashMap::new(),
            loaded: HashMap::new(),
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

    pub(crate) fn record_verdict(
        &mut self,
        candidate: &Candidate,
        failed: bool,
        now: Instant,
        settings: BreakerSettings,
    ) {
        let breaker = self.breaker(candidate, settings);
        if failed {
            breaker.record_failure(now, settings);
        } else {
            breaker.record_healthy(settings);
        }
    }

    pub(crate) fn loaded<C: ?Sized + 'static>(
        &mut self,
        key: &LoadedKey,
        now: Instant,
    ) -> Option<Arc<C>> {
        let entry = self.loaded.get_mut(key)?;
        entry.last_used = now;
        entry.model.downcast_ref::<Arc<C>>().cloned()
    }

    pub(crate) fn store<C: ?Sized + Send + Sync + 'static>(
        &mut self,
        key: LoadedKey,
        model: Arc<C>,
        now: Instant,
    ) {
        let model: Arc<dyn Any + Send + Sync> = Arc::new(model);
        self.loaded.insert(
            key,
            LoadedModel {
                model,
                last_used: now,
            },
        );
    }

    pub(crate) fn forget(&mut self, key: &LoadedKey) {
        self.loaded.remove(key);
    }

    pub(crate) fn unload_idle(
        &mut self,
        now: Instant,
        idle_for: Duration,
        resident: &[Capability],
    ) -> usize {
        let before = self.loaded.len();
        self.loaded.retain(|key, entry| {
            resident.contains(&key.capability)
                || now.saturating_duration_since(entry.last_used) < idle_for
        });
        before - self.loaded.len()
    }
}
