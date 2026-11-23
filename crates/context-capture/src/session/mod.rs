//! The capture session: `ext-image-copy-capture-v1` on the compositor bridge's connection, for
//! the focused window only. Each frame goes into a new memfd that is sealed after the capture
//! and handed to the [`FrameInbox`]. A frame the compositor refuses (a sensitive window) is
//! skipped, never retried.

mod dispatch;
mod state;

use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use context_collect::CompositorLink;
use rustix::event::{EventfdFlags, PollFd, PollFlags, Timespec};
use wayland_client::globals::{BindError, GlobalError, registry_queue_init};
use wayland_client::{Connection, DispatchError, QueueHandle, backend::WaylandError};

use crate::capture_hold::CaptureHolds;
use crate::frame::CaptureTarget;
use crate::frame_inbox::FrameInbox;
use crate::metrics::CaptureMetrics;
use state::SessionState;

/// How long a stop waits for the capture session thread.
const STOP_WAIT: Duration = Duration::from_secs(5);

const IDLE_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("the registry did not answer: {0}")]
    Registry(#[from] GlobalError),
    #[error("a capture global is not offered: {0}")]
    Bind(#[from] BindError),
    #[error("the compositor connection failed: {0}")]
    Dispatch(#[from] DispatchError),
    #[error("the compositor connection failed: {0}")]
    Wayland(#[from] WaylandError),
    #[error("cannot wait for events: {0}")]
    Poll(#[from] rustix::io::Errno),
    #[error("cannot start the capture thread: {0}")]
    Thread(#[from] std::io::Error),
}

#[derive(Debug)]
enum Command {
    Capture(CaptureTarget),
    Abort { done: Sender<()> },
    EndSession,
}

/// Commands for the session thread, which waits on the compositor socket and on `waker`.
#[derive(Debug)]
pub(crate) struct CaptureSession {
    commands: Sender<Command>,
    waker: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureSession {
    /// # Errors
    ///
    /// Fails when the wake-up eventfd or the thread cannot be created.
    pub(crate) fn spawn(
        link: CompositorLink,
        inbox: Arc<FrameInbox>,
        metrics: Arc<CaptureMetrics>,
        holds: Arc<CaptureHolds>,
    ) -> Result<Self, SessionError> {
        let waker = Arc::new(rustix::event::eventfd(
            0,
            EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK,
        )?);
        let (commands, received) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("context-capture".to_owned())
            .spawn({
                let waker = Arc::clone(&waker);
                let stop = Arc::clone(&stop);
                move || {
                    let session = SessionThread {
                        link,
                        inbox,
                        metrics,
                        holds,
                        commands: received,
                        waker,
                        stop,
                    };
                    session.run_forever();
                }
            })?;
        Ok(Self {
            commands,
            waker,
            stop,
            thread: Some(thread),
        })
    }

    pub(crate) fn capture(&self, target: CaptureTarget) {
        self.send(Command::Capture(target));
    }

    /// Gives up the frame in flight and waits up to `timeout` until its memory is zeroed.
    /// Returns whether the session confirmed it.
    pub(crate) fn abort(&self, timeout: Duration) -> bool {
        let (done, confirmed) = mpsc::channel();
        self.send(Command::Abort { done });
        match confirmed.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => false,
        }
    }

    /// Ends the window's session, as when the window loses focus.
    pub(crate) fn end_session(&self) {
        self.send(Command::EndSession);
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            wake(&self.waker);
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        wake(&self.waker);
        if let Some(thread) = self.thread.take()
            && !context_core::join_within(thread, STOP_WAIT)
        {
            tracing::warn!(waited = ?STOP_WAIT, "the capture session thread did not stop; leaving it");
        }
    }
}

fn wake(waker: &OwnedFd) {
    let _ = rustix::io::write(waker, &1_u64.to_ne_bytes());
}

struct SessionThread {
    link: CompositorLink,
    inbox: Arc<FrameInbox>,
    metrics: Arc<CaptureMetrics>,
    holds: Arc<CaptureHolds>,
    commands: Receiver<Command>,
    waker: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
}

impl SessionThread {
    fn run_forever(&self) {
        while !self.stop.load(Ordering::Relaxed) {
            match self.link.connection() {
                Some(connection) => {
                    if let Err(error) = self.run(&connection) {
                        tracing::warn!(%error, "capture session stopped; retrying");
                        self.wait(None, IDLE_WAIT);
                    }
                }
                None => {
                    self.discard_commands();
                    self.wait(None, IDLE_WAIT);
                }
            }
        }
    }

    fn run(&self, connection: &Connection) -> Result<(), SessionError> {
        let (globals, mut queue) = registry_queue_init::<SessionState>(connection)?;
        let handle = queue.handle();
        let mut state = SessionState::bind(
            &globals,
            &handle,
            Arc::clone(&self.inbox),
            Arc::clone(&self.metrics),
        )?;
        queue.roundtrip(&mut state)?;
        tracing::info!("capture session bound");
        let outcome = self.serve(connection, &mut queue, &mut state, &handle);
        state.end_session();
        let _ = queue.flush();
        outcome
    }

    fn serve(
        &self,
        connection: &Connection,
        queue: &mut wayland_client::EventQueue<SessionState>,
        state: &mut SessionState,
        handle: &QueueHandle<SessionState>,
    ) -> Result<(), SessionError> {
        while !self.stop.load(Ordering::Relaxed) {
            if self
                .link
                .connection()
                .is_none_or(|live| live != *connection)
            {
                return Ok(());
            }
            loop {
                match self.commands.try_recv() {
                    Ok(Command::Capture(target)) => state.request(target, handle),
                    Ok(Command::Abort { done }) => {
                        state.abort();
                        let _ = done.send(());
                    }
                    Ok(Command::EndSession) => state.end_session(),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            if self.holds.is_held() {
                state.abort();
            }
            state.progress(handle, Instant::now());
            queue.flush()?;
            queue.dispatch_pending(state)?;
            let timeout = state
                .next_deadline()
                .map_or(IDLE_WAIT, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                })
                .min(IDLE_WAIT);
            if let Some(guard) = queue.prepare_read()
                && self.wait(Some(&guard.connection_fd()), timeout)
            {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            queue.dispatch_pending(state)?;
        }
        Ok(())
    }

    /// Waits for the socket or the waker. Returns whether the socket is readable.
    fn wait(&self, socket: Option<&std::os::fd::BorrowedFd<'_>>, timeout: Duration) -> bool {
        let timeout = Timespec::try_from(timeout).unwrap_or(Timespec {
            tv_sec: 1,
            tv_nsec: 0,
        });
        let waker = self.waker.as_fd();
        let mut fds = vec![PollFd::from_borrowed_fd(waker, PollFlags::IN)];
        if let Some(socket) = socket {
            fds.push(PollFd::from_borrowed_fd(*socket, PollFlags::IN));
        }
        if rustix::event::poll(&mut fds, Some(&timeout)).is_err() {
            return false;
        }
        let woken = fds
            .first()
            .is_some_and(|fd| fd.revents().contains(PollFlags::IN));
        let readable = fds.get(1).is_some_and(|fd| !fd.revents().is_empty());
        if woken {
            let mut counter = [0_u8; 8];
            let _ = rustix::io::read(&*self.waker, &mut counter);
        }
        readable
    }

    fn discard_commands(&self) {
        while let Ok(command) = self.commands.try_recv() {
            if matches!(command, Command::Capture(_)) {
                CaptureMetrics::count(&self.metrics.captures_failed);
            }
        }
    }
}
