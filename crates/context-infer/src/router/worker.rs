use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use crate::error::InferError;
use crate::request::Priority;

/// Work for a model: it gets the loaded model, or the error of its load.
pub(crate) type ModelJob<C> = Box<dyn FnOnce(Result<&C, &InferError>) + Send>;

struct JobQueues<C: ?Sized> {
    interactive: VecDeque<ModelJob<C>>,
    background: VecDeque<ModelJob<C>>,
    stopping: bool,
    abandoned: bool,
    ended: bool,
}

impl<C: ?Sized> JobQueues<C> {
    fn next(&mut self) -> Option<ModelJob<C>> {
        self.interactive
            .pop_front()
            .or_else(|| self.background.pop_front())
    }
}

struct WorkerShared<C: ?Sized> {
    queues: Mutex<JobQueues<C>>,
    wakeup: Condvar,
}

impl<C: ?Sized> WorkerShared<C> {
    fn queues(&self) -> MutexGuard<'_, JobQueues<C>> {
        self.queues.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stop(&self) {
        self.queues().stopping = true;
        self.wakeup.notify_one();
    }
}

/// The thread that loads one model and runs its calls one at a time, interactive first. The
/// model is dropped on this thread, so an unload never blocks the router.
pub(crate) struct ModelWorker<C: ?Sized> {
    shared: Arc<WorkerShared<C>>,
}

/// A way to queue jobs that does not keep the worker alive.
pub(crate) struct JobSender<C: ?Sized> {
    shared: Arc<WorkerShared<C>>,
}

impl<C: ?Sized + Send + Sync + 'static> ModelWorker<C> {
    /// `abandoned_workers` goes down by one when a worker given up as hung ends after all.
    pub(crate) fn spawn(
        name: String,
        load: impl FnOnce() -> Result<Arc<C>, InferError> + Send + 'static,
        abandoned_workers: Arc<AtomicUsize>,
    ) -> Result<Self, InferError> {
        let shared = Arc::new(WorkerShared {
            queues: Mutex::new(JobQueues {
                interactive: VecDeque::new(),
                background: VecDeque::new(),
                stopping: false,
                abandoned: false,
                ended: false,
            }),
            wakeup: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name(name)
            .spawn(move || serve(&worker_shared, load, &abandoned_workers))
            .map_err(|error| {
                InferError::Backend(format!("could not start a model thread: {error}"))
            })?;
        Ok(Self { shared })
    }

    pub(crate) fn sender(&self) -> JobSender<C> {
        JobSender {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Gives up on a hung worker; its thread ends when the hung call returns, if ever. False
    /// when the thread has already ended.
    pub(crate) fn abandon(self) -> bool {
        let mut queues = self.shared.queues();
        queues.abandoned = !queues.ended;
        queues.abandoned
    }
}

impl<C: ?Sized> Drop for ModelWorker<C> {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

impl<C: ?Sized> JobSender<C> {
    pub(crate) fn send(&self, priority: Priority, job: ModelJob<C>) {
        let mut queues = self.shared.queues();
        match priority {
            Priority::Interactive => queues.interactive.push_back(job),
            Priority::Background => queues.background.push_back(job),
        }
        drop(queues);
        self.shared.wakeup.notify_one();
    }
}

fn serve<C: ?Sized>(
    shared: &WorkerShared<C>,
    load: impl FnOnce() -> Result<Arc<C>, InferError>,
    abandoned_workers: &AtomicUsize,
) {
    let _end = WorkerEnd {
        shared,
        abandoned_workers,
    };
    let model = load();
    let model = model.as_deref();
    loop {
        let job = {
            let mut queues = shared.queues();
            loop {
                if let Some(job) = queues.next() {
                    break job;
                }
                if queues.stopping {
                    return;
                }
                queues = shared
                    .wakeup
                    .wait(queues)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        job(model);
    }
}

/// Runs when the worker thread ends, also by a panic: drops the queued jobs, so that their
/// callers stop waiting, and settles the count of abandoned workers.
struct WorkerEnd<'a, C: ?Sized> {
    shared: &'a WorkerShared<C>,
    abandoned_workers: &'a AtomicUsize,
}

impl<C: ?Sized> Drop for WorkerEnd<'_, C> {
    fn drop(&mut self) {
        let mut queues = self.shared.queues();
        queues.stopping = true;
        queues.interactive.clear();
        queues.background.clear();
        queues.ended = true;
        if queues.abandoned {
            self.abandoned_workers.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
