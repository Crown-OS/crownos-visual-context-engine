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
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crownos_config::system::ai::{Device, Runtime};

use crate::capability::{
    Embed, EmbedPurpose, EmbedSpace, Embeddings, Generate, Ocr, Rerank, TextLine, Transcribe,
};
use crate::error::InferError;
use crate::media::Image;
use crate::registry::{Candidate, Capability, Registry};
use crate::request::JobCancel;
use crate::request::{GenRequest, Generation, Job};
use crate::task::Task;

pub use backend::{LoadSpec, RuntimeBackend};
pub use benchmark::{BenchmarkCache, benchmark_fingerprint, default_benchmark_path};
pub use breaker::BreakerSettings;
pub use policy::Policy;

use breaker::Verdict;
use loadable::Loadable;
use state::{LoadedKey, RouterState};
use worker::{JobSender, ModelJob, ModelWorker};

/// A call to a loaded model that takes this many times its measured latency, and at least
/// [`MIN_HANG_TIME`], counts as hung, so the router can still try the next candidate.
const HANG_FACTOR: u32 = 4;
const MIN_HANG_TIME: Duration = Duration::from_secs(1);

/// Hung model threads that may still hold memory. Above this, the router loads no new model;
/// a restart of the service frees them.
const MAX_ABANDONED_WORKERS: usize = 2;
/// A caller that finds its model loading for another caller tries the next candidate.
const LOADING_RETRY: Duration = Duration::from_millis(500);
/// How often a wait for a model looks at the job's cancel token.
const CANCEL_CHECK: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub struct RouterSettings {
    pub models_dir: PathBuf,
    pub policy: Policy,
    pub benchmarks: BenchmarkCache,
    pub breaker: BreakerSettings,
    /// Extra time a call gets past its deadline to stop by itself before the watchdog gives
    /// up on it.
    pub watchdog_grace: Duration,
    /// A call with no measured latency counts as hung after this long. A call that only
    /// outlasts its caller's deadline keeps running and keeps its model.
    pub unmeasured_call_limit: Duration,
    /// A model load counts as hung after this long.
    pub load_limit: Duration,
}

impl RouterSettings {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            policy: Policy::default(),
            benchmarks: BenchmarkCache::default(),
            breaker: BreakerSettings::default(),
            watchdog_grace: Duration::from_millis(250),
            unmeasured_call_limit: Duration::from_secs(600),
            load_limit: Duration::from_secs(300),
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
    unmeasured_call_limit: Duration,
    load_limit: Duration,
    abandoned_workers: Arc<AtomicUsize>,
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

/// What a model thread reports about one call.
enum CallEvent<R> {
    LoadFailed(InferError),
    Started(Instant),
    Finished(Result<R, InferError>, Duration),
}

/// What happens to a model thread after a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerFate {
    Keep,
    Remove,
    Abandon,
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
            unmeasured_call_limit: settings.unmeasured_call_limit,
            load_limit: settings.load_limit,
            abandoned_workers: Arc::new(AtomicUsize::new(0)),
            state: Mutex::new(RouterState::new(settings.policy, settings.benchmarks)),
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// True when so many model threads hang that only a restart of the process frees them.
    pub fn needs_restart(&self) -> bool {
        self.abandoned_workers.load(Ordering::SeqCst) >= MAX_ABANDONED_WORKERS
    }

    pub fn policy(&self) -> Policy {
        self.lock().policy
    }

    pub fn set_policy(&self, policy: Policy) {
        self.lock().policy = policy;
    }

    /// A copy of the latency cache, to save.
    pub fn benchmarks(&self) -> BenchmarkCache {
        self.lock().benchmarks.clone()
    }

    /// The (runtime, device) pairs whose probe passed.
    pub fn available_devices(&self) -> impl Iterator<Item = (Runtime, Device)> + '_ {
        self.available.iter().copied()
    }

    /// The models loaded now, as (capability, model, runtime, device).
    pub fn loaded_models(&self) -> Vec<(Capability, String, Runtime, Device)> {
        self.lock().loaded_models()
    }

    /// The candidates a call for `task` would try now, best first.
    pub fn routes(&self, task: Task) -> Vec<Candidate> {
        self.ranked(task)
    }

    /// The space `embed` uses now. A store that holds vectors from another space must
    /// embed its content again.
    pub fn embed_space(&self) -> Option<EmbedSpace> {
        let first = self.ranked(Task::EmbedText).into_iter().next()?;
        self.registry.embed_space(&first)
    }

    /// Unloads the models not used for `idle_for`, except those with a `resident` capability.
    pub fn unload_idle(&self, idle_for: Duration, resident: &[Capability]) -> usize {
        let unloaded = self
            .lock()
            .take_idle_workers(Instant::now(), idle_for, resident);
        unloaded.len()
    }

    pub fn unload_all(&self) -> usize {
        self.unload_idle(Duration::ZERO, &[])
    }

    pub fn generate(&self, task: Task, request: GenRequest) -> Result<Generation, InferError> {
        let job = request.job.clone();
        let units = u64::from(request.max_tokens);
        let request = Arc::new(request);
        self.run::<dyn Generate, _>(task, &job, units, move |generator| {
            generator.generate(&request)
        })
        .map(|(generation, _)| generation)
    }

    pub fn embed(
        &self,
        texts: Vec<String>,
        purpose: EmbedPurpose,
        job: &Job,
    ) -> Result<EmbeddedTexts, InferError> {
        let units = texts.iter().map(|text| text.len() as u64).sum();
        let ((reported_space, embeddings), candidate) =
            self.run::<dyn Embed, _>(Task::EmbedText, job, units, move |embedder| {
                Ok((embedder.space().clone(), embedder.embed(&texts, purpose)?))
            })?;
        let space = self.registry.embed_space(&candidate).ok_or_else(|| {
            InferError::Unavailable(format!("the space of {}", candidate.model).into())
        })?;
        if reported_space != space || embeddings.dimensions != space.dimensions as usize {
            return Err(InferError::Backend(format!(
                "{} gave {reported_space} with {} dimensions, not {space}",
                candidate.model, embeddings.dimensions
            )));
        }
        Ok(EmbeddedTexts { space, embeddings })
    }

    pub fn rerank(
        &self,
        query: String,
        documents: Vec<String>,
        job: &Job,
    ) -> Result<Vec<f32>, InferError> {
        let units = documents
            .iter()
            .map(|document| (query.len() + document.len()) as u64)
            .sum();
        self.run::<dyn Rerank, _>(Task::Rerank, job, units, move |reranker| {
            reranker.rerank(&query, &documents)
        })
        .map(|(scores, _)| scores)
    }

    pub fn recognize_text(&self, image: Image, job: &Job) -> Result<Vec<TextLine>, InferError> {
        let units = u64::from(image.width()) * u64::from(image.height());
        self.run::<dyn Ocr, _>(Task::RecognizeText, job, units, move |ocr| {
            ocr.recognize(&image)
        })
        .map(|(lines, _)| lines)
    }

    pub fn transcribe(&self, samples: Arc<[i16]>, job: &Job) -> Result<String, InferError> {
        let units = samples.len() as u64;
        self.run::<dyn Transcribe, _>(Task::Transcribe, job, units, move |transcriber| {
            transcriber.transcribe(&samples)
        })
        .map(|(text, _)| text)
    }

    fn lock(&self) -> MutexGuard<'_, RouterState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The candidates to try, best first. The registry order holds across models and
    /// runtimes; measured latency orders only the devices inside one run of the same model
    /// and runtime. Embedding stays in the space of the first candidate, so a passing fault
    /// never mixes vectors of two spaces.
    fn ranked(&self, task: Task) -> Vec<Candidate> {
        let state = self.lock();
        let mut ranked: Vec<Candidate> = self
            .registry
            .candidates(task)
            .iter()
            .filter(|candidate| {
                self.available
                    .contains(&(candidate.runtime, candidate.device))
                    && state.policy.permits(candidate)
            })
            .cloned()
            .collect();
        for run in ranked
            .chunk_by_mut(|left, right| left.model == right.model && left.runtime == right.runtime)
        {
            let cost = |candidate: &Candidate| state.benchmarks.nanos_per_unit(task, candidate);
            if run.iter().all(|candidate| cost(candidate).is_some()) {
                run.sort_by_cached_key(|candidate| cost(candidate).map(f64::to_bits));
            }
        }
        if task == Task::EmbedText {
            let first_space = ranked
                .first()
                .and_then(|candidate| self.registry.embed_space(candidate));
            ranked.retain(|candidate| self.registry.embed_space(candidate) == first_space);
        }
        ranked
    }

    fn run<C, R>(
        &self,
        task: Task,
        job: &Job,
        units: u64,
        call: impl Fn(&C) -> Result<R, InferError> + Send + Sync + 'static,
    ) -> Result<(R, Candidate), InferError>
    where
        C: Loadable + ?Sized,
        R: Send + 'static,
    {
        let call = Arc::new(call);
        let mut last_error = InferError::Unavailable(format!("a candidate for {task:?}").into());
        for candidate in self.ranked(task) {
            if job.cancel.is_cancelled() {
                return Err(InferError::Cancelled);
            }
            let now = Instant::now();
            if now >= job.deadline {
                return Err(InferError::Timeout);
            }
            let expected = task
                .has_predictable_cost()
                .then(|| self.lock().benchmarks.latency(task, &candidate, units))
                .flatten();
            if expected.is_some_and(|latency| now + latency > job.deadline) {
                last_error = InferError::Timeout;
                continue;
            }
            let permit = self
                .lock()
                .breaker(&candidate, self.breaker_settings)
                .try_acquire(now);
            if let Err(retry_after) = permit {
                last_error = InferError::Busy { retry_after };
                continue;
            }
            let call = Arc::clone(&call);
            let attempt = Attempt {
                task,
                candidate: &candidate,
                job: job.clone(),
                units,
                expected,
            };
            match self.attempt::<C, R>(&attempt, move |model| call(model)) {
                Ok(value) => return Ok((value, candidate)),
                Err(error @ InferError::InvalidInput(_)) => return Err(error),
                Err(error) => {
                    tracing::debug!(?task, ?candidate, %error, "falling through");
                    last_error = error;
                }
            }
        }
        Err(last_error)
    }

    /// One call on one candidate, on the model's own thread. A wait that ends in the load, in
    /// the queue or at the caller's deadline is inconclusive; a call that passes its hang
    /// budget means the model hung, also when a later caller finds it so.
    fn attempt<C, R>(
        &self,
        attempt: &Attempt<'_>,
        call: impl FnOnce(&C) -> Result<R, InferError> + Send + 'static,
    ) -> Result<R, InferError>
    where
        C: Loadable + ?Sized,
        R: Send + 'static,
    {
        let key = LoadedKey::new(C::CAPABILITY, attempt.candidate);
        let (sender, started_the_load) = match self.worker_sender::<C>(&key, attempt.candidate) {
            Ok(worker) => worker,
            Err(error) => {
                self.settle::<C>(attempt, &key, Verdict::Inconclusive, WorkerFate::Keep, None);
                return Err(error);
            }
        };
        if sender.is_hung(Instant::now()) {
            self.settle::<C>(attempt, &key, Verdict::Failed, WorkerFate::Abandon, None);
            return Err(InferError::Backend(
                "the model hangs in its load or in an earlier call".into(),
            ));
        }
        if sender.is_loading() && !started_the_load {
            self.settle::<C>(attempt, &key, Verdict::Inconclusive, WorkerFate::Keep, None);
            return Err(InferError::Busy {
                retry_after: LOADING_RETRY,
            });
        }
        let hang_budget = attempt
            .expected
            .map_or(self.unmeasured_call_limit, |latency| {
                (latency * HANG_FACTOR).max(MIN_HANG_TIME)
            });
        let (events, received) = mpsc::sync_channel(2);
        let deadline = attempt.job.deadline;
        let cancel = attempt.job.cancel.clone();
        let run = Box::new(move |model: Result<&C, &InferError>| {
            let model = match model {
                Ok(model) => model,
                Err(load_error) => {
                    let _ = events.send(CallEvent::LoadFailed(load_error.clone()));
                    return;
                }
            };
            if cancel.is_cancelled() {
                let _ = events.send(CallEvent::Finished(
                    Err(InferError::Cancelled),
                    Duration::ZERO,
                ));
                return;
            }
            if Instant::now() >= deadline {
                let _ = events.send(CallEvent::Finished(
                    Err(InferError::Timeout),
                    Duration::ZERO,
                ));
                return;
            }
            let started = Instant::now();
            let _ = events.send(CallEvent::Started(started));
            let result = call(model);
            let _ = events.send(CallEvent::Finished(result, started.elapsed()));
        });
        if let Err(error) = sender.send(attempt.job.priority, ModelJob { hang_budget, run }) {
            let fate = if matches!(error, InferError::Busy { .. }) {
                WorkerFate::Keep
            } else {
                WorkerFate::Remove
            };
            self.settle::<C>(attempt, &key, Verdict::Inconclusive, fate, None);
            return Err(error);
        }
        let cancel = &attempt.job.cancel;
        let receive =
            |limit: Instant| receive_until(&received, limit + self.watchdog_grace, cancel);
        let mut event = receive(deadline);
        if let Ok(CallEvent::Started(started)) = event {
            let hang_at = started + hang_budget;
            event = receive(deadline.min(hang_at));
            if let Err(RecvTimeoutError::Timeout) = event
                && !cancel.is_cancelled()
            {
                let (verdict, fate) = if hang_at <= deadline {
                    (Verdict::Failed, WorkerFate::Abandon)
                } else {
                    (Verdict::Inconclusive, WorkerFate::Keep)
                };
                self.settle::<C>(attempt, &key, verdict, fate, None);
                return Err(InferError::Timeout);
            }
        }
        if cancel.is_cancelled() && !matches!(event, Ok(CallEvent::Finished(..))) {
            self.settle::<C>(attempt, &key, Verdict::Inconclusive, WorkerFate::Keep, None);
            return Err(InferError::Cancelled);
        }
        let (result, verdict, fate, elapsed) = match event {
            Ok(CallEvent::Finished(Ok(value), elapsed)) => {
                (Ok(value), Verdict::Healthy, WorkerFate::Keep, Some(elapsed))
            }
            Ok(CallEvent::Finished(Err(error @ InferError::Backend(_)), _)) => {
                (Err(error), Verdict::Failed, WorkerFate::Remove, None)
            }
            Ok(CallEvent::Finished(Err(error), _)) => {
                (Err(error), Verdict::Inconclusive, WorkerFate::Keep, None)
            }
            Ok(CallEvent::LoadFailed(error)) => {
                let verdict = if matches!(error, InferError::Backend(_)) {
                    Verdict::Failed
                } else {
                    Verdict::Inconclusive
                };
                (Err(error), verdict, WorkerFate::Remove, None)
            }
            Err(RecvTimeoutError::Timeout) => (
                Err(InferError::Timeout),
                Verdict::Inconclusive,
                WorkerFate::Keep,
                None,
            ),
            Ok(CallEvent::Started(_)) | Err(RecvTimeoutError::Disconnected) => {
                let error = InferError::Backend("the model thread stopped during a call".into());
                (Err(error), Verdict::Failed, WorkerFate::Remove, None)
            }
        };
        self.settle::<C>(attempt, &key, verdict, fate, elapsed);
        result
    }

    /// The model's thread, started now when there is none; true when this call started it.
    fn worker_sender<C: Loadable + ?Sized>(
        &self,
        key: &LoadedKey,
        candidate: &Candidate,
    ) -> Result<(JobSender<C>, bool), InferError> {
        let mut state = self.lock();
        if let Some(sender) = state.sender::<C>(key, Instant::now()) {
            return Ok((sender, false));
        }
        if self.abandoned_workers.load(Ordering::SeqCst) >= MAX_ABANDONED_WORKERS {
            return Err(InferError::Unavailable(
                "a new model: too many hung models; restart the service".into(),
            ));
        }
        let backend = self.backends.get(&candidate.runtime).cloned();
        let registry = Arc::clone(&self.registry);
        let models_dir = Arc::clone(&self.models_dir);
        let owned_candidate = candidate.clone();
        let worker = ModelWorker::spawn(
            format!("model {}", candidate.model),
            move || load::<C>(backend.as_deref(), &registry, &owned_candidate, &models_dir),
            self.load_limit,
            Arc::clone(&self.abandoned_workers),
        )?;
        let sender = worker.sender();
        state.insert_worker(key.clone(), worker, Instant::now());
        Ok((sender, true))
    }

    /// Records the verdict and the latency, then drops or abandons the model thread after
    /// the router lock is released.
    fn settle<C: Loadable + ?Sized>(
        &self,
        attempt: &Attempt<'_>,
        key: &LoadedKey,
        verdict: Verdict,
        fate: WorkerFate,
        elapsed: Option<Duration>,
    ) {
        let now = Instant::now();
        let mut state = self.lock();
        if let Some(elapsed) = elapsed {
            state
                .benchmarks
                .record(attempt.task, attempt.candidate, elapsed, attempt.units);
            state.touch(key, now);
        }
        state
            .breaker(attempt.candidate, self.breaker_settings)
            .record(verdict, now, self.breaker_settings);
        let removed = match fate {
            WorkerFate::Keep => None,
            WorkerFate::Remove | WorkerFate::Abandon => state.take_worker::<C>(key),
        };
        drop(state);
        if let Some(worker) = removed
            && fate == WorkerFate::Abandon
        {
            self.abandoned_workers.fetch_add(1, Ordering::SeqCst);
            if !worker.abandon() {
                self.abandoned_workers.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

struct Attempt<'a> {
    task: Task,
    candidate: &'a Candidate,
    job: Job,
    units: u64,
    expected: Option<Duration>,
}

/// The next event of a call, until `limit`. A cancelled job stops the wait within
/// [`CANCEL_CHECK`] and reads as a timeout.
fn receive_until<T>(
    received: &mpsc::Receiver<T>,
    limit: Instant,
    cancel: &JobCancel,
) -> Result<T, RecvTimeoutError> {
    loop {
        let left = limit.saturating_duration_since(Instant::now());
        if left.is_zero() || cancel.is_cancelled() {
            return Err(RecvTimeoutError::Timeout);
        }
        match received.recv_timeout(left.min(CANCEL_CHECK)) {
            Err(RecvTimeoutError::Timeout) => {}
            other => return other,
        }
    }
}

fn load<C: Loadable + ?Sized>(
    backend: Option<&dyn RuntimeBackend>,
    registry: &Registry,
    candidate: &Candidate,
    models_dir: &Path,
) -> Result<Arc<C>, InferError> {
    let backend = backend
        .ok_or_else(|| InferError::Unavailable(format!("{:?}", candidate.runtime).into()))?;
    C::load(backend, &LoadSpec::new(registry, candidate, models_dir)?)
}
