//! Picks a candidate for each call: probe results, policy, breakers and cached latency decide
//! which (model, runtime, device) runs, and a failure falls through to the next candidate.

mod backend;
mod benchmark;
mod breaker;
mod loadable;
mod policy;
mod state;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crownos_config::system::ai::{Device, Runtime};

use crate::capability::{
    Embed, EmbedPurpose, EmbedSpace, Embeddings, Generate, Ocr, Rerank, TextLine, Transcribe,
};
use crate::error::InferError;
use crate::media::RgbImage;
use crate::registry::{Candidate, Capability, Registry};
use crate::request::{GenRequest, Generation, Job};
use crate::task::Task;
use crate::util::watchdog::run_until;

pub use backend::{LoadSpec, RuntimeBackend};
pub use benchmark::{BenchmarkCache, benchmark_fingerprint, default_benchmark_path};
pub use breaker::BreakerSettings;
pub use policy::Policy;

use loadable::Loadable;
use state::{LoadedKey, RouterState};

/// A call to a loaded model that takes this many times its measured latency, and at least
/// [`MIN_HANG_TIME`], counts as hung, so the router can still try the next candidate.
const HANG_FACTOR: u32 = 4;
const MIN_HANG_TIME: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct RouterSettings {
    pub models_dir: PathBuf,
    pub policy: Policy,
    pub benchmarks: BenchmarkCache,
    pub breaker: BreakerSettings,
    /// Extra time a call gets past its deadline to stop by itself before the watchdog gives
    /// up on it.
    pub watchdog_grace: Duration,
}

impl RouterSettings {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            policy: Policy::default(),
            benchmarks: BenchmarkCache::default(),
            breaker: BreakerSettings::default(),
            watchdog_grace: Duration::from_millis(250),
        }
    }
}

/// Vectors with the space they belong to.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedTexts {
    pub space: EmbedSpace,
    pub embeddings: Embeddings,
}

pub struct Router {
    registry: Arc<Registry>,
    backends: HashMap<Runtime, Arc<dyn RuntimeBackend>>,
    available: HashSet<(Runtime, Device)>,
    models_dir: Arc<Path>,
    breaker_settings: BreakerSettings,
    watchdog_grace: Duration,
    state: Mutex<RouterState>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Router")
            .field("available", &self.available)
            .field("models_dir", &self.models_dir)
            .finish_non_exhaustive()
    }
}

impl Router {
    /// Probes each (runtime, device) that the registry names, once. No model loads here.
    pub fn new(
        registry: Registry,
        backends: Vec<Arc<dyn RuntimeBackend>>,
        settings: RouterSettings,
    ) -> Self {
        let backends: HashMap<Runtime, Arc<dyn RuntimeBackend>> = backends
            .into_iter()
            .map(|backend| (backend.runtime(), backend))
            .collect();
        let available = registry
            .tasks
            .values()
            .flatten()
            .map(|candidate| (candidate.runtime, candidate.device))
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|&(runtime, device)| {
                backends
                    .get(&runtime)
                    .is_some_and(|backend| backend.probe(device))
            })
            .collect();
        Self {
            registry: Arc::new(registry),
            backends,
            available,
            models_dir: Arc::from(settings.models_dir),
            breaker_settings: settings.breaker,
            watchdog_grace: settings.watchdog_grace,
            state: Mutex::new(RouterState::new(settings.policy, settings.benchmarks)),
        }
    }

    pub fn set_policy(&self, policy: Policy) {
        self.lock().policy = policy;
    }

    /// A copy of the latency cache, to save.
    pub fn benchmarks(&self) -> BenchmarkCache {
        self.lock().benchmarks.clone()
    }

    /// The space `embed` uses now. A store that holds vectors from another space must
    /// embed its content again.
    pub fn embed_space(&self) -> Option<EmbedSpace> {
        let first = self.ranked(Task::EmbedText).into_iter().next()?;
        self.registry.embed_space(&first.model)
    }

    /// Drops the models not used for `idle_for`, except those with a `resident` capability.
    pub fn unload_idle(&self, idle_for: Duration, resident: &[Capability]) -> usize {
        self.lock().unload_idle(Instant::now(), idle_for, resident)
    }

    pub fn unload_all(&self) -> usize {
        self.unload_idle(Duration::ZERO, &[])
    }

    pub fn generate(&self, task: Task, request: GenRequest) -> Result<Generation, InferError> {
        let deadline = request.job.deadline;
        let request = Arc::new(request);
        self.run::<dyn Generate, _>(task, deadline, move |generator| {
            generator.generate(&request)
        })
        .map(|(generation, _)| generation)
    }

    pub fn embed(
        &self,
        texts: Vec<String>,
        purpose: EmbedPurpose,
        job: Job,
    ) -> Result<EmbeddedTexts, InferError> {
        let (embeddings, candidate) =
            self.run::<dyn Embed, _>(Task::EmbedText, job.deadline, move |embedder| {
                embedder.embed(&texts, purpose)
            })?;
        let space = self.registry.embed_space(&candidate.model).ok_or_else(|| {
            InferError::Unavailable(format!("space of {}", candidate.model).into())
        })?;
        if embeddings.dimensions != space.dimensions as usize {
            return Err(InferError::Backend(format!(
                "{} gave {} dimensions, not {}",
                candidate.model, embeddings.dimensions, space.dimensions
            )));
        }
        Ok(EmbeddedTexts { space, embeddings })
    }

    pub fn rerank(
        &self,
        query: String,
        documents: Vec<String>,
        job: Job,
    ) -> Result<Vec<f32>, InferError> {
        self.run::<dyn Rerank, _>(Task::Rerank, job.deadline, move |reranker| {
            reranker.rerank(&query, &documents)
        })
        .map(|(scores, _)| scores)
    }

    pub fn recognize_text(&self, image: RgbImage, job: Job) -> Result<Vec<TextLine>, InferError> {
        self.run::<dyn Ocr, _>(Task::RecognizeText, job.deadline, move |ocr| {
            ocr.recognize(&image)
        })
        .map(|(lines, _)| lines)
    }

    pub fn transcribe(&self, samples: Arc<[i16]>, job: Job) -> Result<String, InferError> {
        self.run::<dyn Transcribe, _>(Task::Transcribe, job.deadline, move |transcriber| {
            transcriber.transcribe(&samples)
        })
        .map(|(text, _)| text)
    }

    fn lock(&self) -> MutexGuard<'_, RouterState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The candidates to try, best first. The devices of one model are ordered by measured
    /// latency once all of them have a measurement. Embedding stays in the space of the
    /// first candidate, so a passing fault never mixes vectors of two spaces.
    fn ranked(&self, task: Task) -> Vec<Candidate> {
        let state = self.lock();
        let eligible: Vec<&Candidate> = self
            .registry
            .candidates(task)
            .iter()
            .filter(|candidate| {
                self.available
                    .contains(&(candidate.runtime, candidate.device))
                    && state.policy.permits(candidate)
            })
            .collect();
        let model_rank = |model: &str| {
            eligible
                .iter()
                .position(|candidate| candidate.model == model)
        };
        let fully_measured = |model: &str| {
            eligible
                .iter()
                .filter(|candidate| candidate.model == model)
                .all(|candidate| state.benchmarks.latency(task, candidate).is_some())
        };
        let mut ranked: Vec<(usize, &Candidate)> = eligible.iter().copied().enumerate().collect();
        ranked.sort_by_key(|&(index, candidate)| {
            let latency = fully_measured(&candidate.model)
                .then(|| state.benchmarks.latency(task, candidate))
                .flatten();
            (model_rank(&candidate.model), latency, index)
        });
        let first_model = ranked
            .first()
            .map(|(_, candidate)| candidate.model.as_str());
        ranked
            .iter()
            .filter(|(_, candidate)| {
                task != Task::EmbedText || Some(candidate.model.as_str()) == first_model
            })
            .map(|&(_, candidate)| candidate.clone())
            .collect()
    }

    fn run<C, R>(
        &self,
        task: Task,
        deadline: Instant,
        call: impl Fn(&C) -> Result<R, InferError> + Send + Sync + 'static,
    ) -> Result<(R, Candidate), InferError>
    where
        C: Loadable + ?Sized,
        R: Send + 'static,
    {
        let call = Arc::new(call);
        let mut last_error = InferError::Unavailable(format!("a candidate for {task:?}").into());
        for candidate in self.ranked(task) {
            let now = Instant::now();
            if now >= deadline {
                return Err(InferError::Timeout);
            }
            let expected = self.lock().benchmarks.latency(task, &candidate);
            if expected.is_some_and(|latency| now + latency > deadline) {
                last_error = InferError::Timeout;
                continue;
            }
            if !self
                .lock()
                .breaker(&candidate, self.breaker_settings)
                .try_acquire(now)
            {
                continue;
            }
            let call = Arc::clone(&call);
            match self.attempt::<C, R>(task, &candidate, deadline, expected, move |model| {
                call(model)
            }) {
                Ok(value) => return Ok((value, candidate)),
                Err(error) => {
                    tracing::debug!(?task, ?candidate, %error, "falling through");
                    last_error = error;
                }
            }
        }
        Err(last_error)
    }

    /// One call on one candidate: load it when needed, then call it under the watchdog.
    fn attempt<C, R>(
        &self,
        task: Task,
        candidate: &Candidate,
        deadline: Instant,
        expected: Option<Duration>,
        call: impl FnOnce(&C) -> Result<R, InferError> + Send + 'static,
    ) -> Result<R, InferError>
    where
        C: Loadable + ?Sized,
        R: Send + 'static,
    {
        let key = LoadedKey::new(C::CAPABILITY, candidate);
        let now = Instant::now();
        let cached: Option<Arc<C>> = self.lock().loaded(&key, now);
        let call_deadline = match (&cached, expected) {
            (Some(_), Some(latency)) => {
                deadline.min(now + (latency * HANG_FACTOR).max(MIN_HANG_TIME))
            }
            _ => deadline,
        };
        let backend = self.backends.get(&candidate.runtime).cloned();
        let registry = Arc::clone(&self.registry);
        let models_dir = Arc::clone(&self.models_dir);
        let owned_candidate = candidate.clone();
        let outcome = run_until(call_deadline + self.watchdog_grace, move || {
            let model = match cached {
                Some(model) => model,
                None => {
                    let backend = backend.ok_or_else(|| {
                        InferError::Unavailable(format!("{:?}", owned_candidate.runtime).into())
                    })?;
                    let spec = LoadSpec::new(&registry, &owned_candidate, &models_dir)?;
                    C::load(backend.as_ref(), &spec)?
                }
            };
            let started = Instant::now();
            let result = call(&model);
            Ok((model, result, started.elapsed()))
        });
        let (model, result) = match outcome.and_then(|finished| finished) {
            Ok((model, result, elapsed)) => {
                if result.is_ok() {
                    self.lock().benchmarks.record(task, candidate, elapsed);
                }
                (Some(model), result)
            }
            Err(error) => (None, Err(error)),
        };
        let failed = result.as_ref().is_err_and(InferError::trips_breaker);
        let mut state = self.lock();
        match model {
            Some(model) if !failed => state.store(key, model, Instant::now()),
            _ => state.forget(&key),
        }
        state.record_verdict(candidate, failed, Instant::now(), self.breaker_settings);
        result
    }
}
