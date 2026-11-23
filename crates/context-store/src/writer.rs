//! One thread owns the write connection. Writes queue on a channel and land in one transaction
//! per batch, so a burst of observations costs one fsync.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use context_core::{Episode, FrameRecord, Observation, Segment, SegmentId, TimeRange, Timestamp};

use crate::queries::{self, ForgetOutcome, TextLine};
use crate::store::{Store, StoreError};

/// How long a stop waits for the store writer thread.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// The most a batch waits before it commits.
const COALESCE: Duration = Duration::from_millis(250);
const QUEUE_DEPTH: usize = 4_096;

/// One thing to persist.
#[derive(Debug)]
pub enum Write {
    App {
        app_key: String,
        app_id: String,
        name: String,
    },
    Event {
        observation: Observation,
        app_key: Option<String>,
        t_end: Timestamp,
    },
    Segment(Box<Segment>),
    Episode(Box<Episode>),
    TextLines {
        segment: SegmentId,
        lines: Vec<TextLine>,
    },
    /// The metadata of one analysed capture. It has no pixel and no path.
    Frame {
        segment: SegmentId,
        frame: FrameRecord,
        t: Timestamp,
    },
    WorkingMemory {
        snapshot: Vec<u8>,
        t: Timestamp,
    },
    Audit {
        actor: String,
        action: String,
        detail: String,
        t: Timestamp,
    },
    Forget {
        range: TimeRange,
        app_key: Option<String>,
        reply: mpsc::Sender<Result<ForgetOutcome, String>>,
    },
    /// Commits what is queued and answers once it is durable.
    Flush(mpsc::Sender<()>),
}

#[derive(Debug, Clone)]
pub struct WriteHandle {
    sender: SyncSender<Write>,
}

impl WriteHandle {
    /// # Errors
    ///
    /// Fails once the writer thread has ended.
    pub fn send(&self, write: Write) -> Result<(), StoreError> {
        self.sender.send(write).map_err(|_| StoreError::WriterGone)
    }

    /// Waits until everything sent before is committed.
    ///
    /// # Errors
    ///
    /// Fails once the writer thread has ended.
    pub fn flush(&self) -> Result<(), StoreError> {
        let (reply, done) = mpsc::channel();
        self.send(Write::Flush(reply))?;
        done.recv().map_err(|_| StoreError::WriterGone)
    }

    /// Forgets a range and waits for the counts.
    ///
    /// # Errors
    ///
    /// Fails once the writer thread has ended or the deletion failed.
    pub fn forget(
        &self,
        range: TimeRange,
        app_key: Option<String>,
    ) -> Result<ForgetOutcome, StoreError> {
        let (reply, outcome) = mpsc::channel();
        self.send(Write::Forget {
            range,
            app_key,
            reply,
        })?;
        outcome
            .recv()
            .map_err(|_| StoreError::WriterGone)?
            .map_err(StoreError::Forget)
    }
}

#[derive(Debug)]
pub struct Writer {
    thread: Option<JoinHandle<()>>,
    handle: WriteHandle,
}

impl Writer {
    /// Takes the store's connection onto its own thread.
    ///
    /// # Errors
    ///
    /// Fails when the thread cannot be spawned.
    pub fn spawn(store: Store) -> Result<Self, std::io::Error> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_DEPTH);
        let thread = std::thread::Builder::new()
            .name("context-writer".to_owned())
            .spawn(move || run(store, &receiver))?;
        Ok(Self {
            thread: Some(thread),
            handle: WriteHandle { sender },
        })
    }

    pub fn handle(&self) -> WriteHandle {
        self.handle.clone()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.handle.flush();
        if let Some(thread) = self.thread.take() {
            drop(std::mem::replace(
                &mut self.handle,
                WriteHandle {
                    sender: mpsc::sync_channel(1).0,
                },
            ));
            if !context_core::join_within(thread, STOP_WAIT) {
                tracing::warn!(waited = ?STOP_WAIT, "the store writer thread did not stop; leaving it");
            }
        }
    }
}

fn run(mut store: Store, receiver: &Receiver<Write>) {
    loop {
        let Ok(first) = receiver.recv() else {
            return;
        };
        let mut batch = vec![first];
        let deadline = Instant::now() + COALESCE;
        let mut flushes = Vec::new();
        loop {
            if matches!(batch.last(), Some(Write::Flush(_) | Write::Forget { .. })) {
                break;
            }
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(write) => batch.push(write),
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if let Err(error) = apply(&mut store, batch, &mut flushes) {
            tracing::error!(%error, "a batch of writes was rolled back");
        }
        for flush in flushes {
            let _ = flush.send(());
        }
    }
}

fn apply(
    store: &mut Store,
    batch: Vec<Write>,
    flushes: &mut Vec<mpsc::Sender<()>>,
) -> Result<(), StoreError> {
    let connection = store.connection_mut();
    let transaction = connection.transaction()?;
    let mut forgets = Vec::new();
    for write in batch {
        match write {
            Write::App {
                app_key,
                app_id,
                name,
            } => {
                queries::upsert_app(&transaction, &app_key, &app_id, &name)?;
            }
            Write::Event {
                observation,
                app_key,
                t_end,
            } => {
                queries::insert_event(&transaction, &observation, app_key.as_deref(), t_end)?;
            }
            Write::Segment(segment) => queries::upsert_segment(&transaction, &segment)?,
            Write::Episode(episode) => queries::upsert_episode(&transaction, &episode)?,
            Write::TextLines { segment, lines } => {
                queries::insert_text_lines(&transaction, segment, &lines)?;
            }
            Write::Frame { segment, frame, t } => {
                queries::insert_frame(&transaction, segment, &frame, t)?;
            }
            Write::WorkingMemory { snapshot, t } => {
                queries::save_working_memory(&transaction, &snapshot, t)?;
            }
            Write::Audit {
                actor,
                action,
                detail,
                t,
            } => queries::audit(&transaction, &actor, &action, &detail, t)?,
            Write::Forget {
                range,
                app_key,
                reply,
            } => {
                let outcome = queries::forget(&transaction, range, app_key.as_deref());
                forgets.push((reply, outcome));
            }
            Write::Flush(reply) => flushes.push(reply),
        }
    }
    transaction.commit()?;
    for (reply, outcome) in forgets {
        let _ = reply.send(outcome.map_err(|error| error.to_string()));
    }
    Ok(())
}
