//! One connection to crown-context-inferd for the whole process, shared by every caller, so
//! jobs and their cancels go out on it together. A lost connection is made again.

use std::os::fd::AsFd;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use context_infer::InferError;
use context_protocol::inference::Client;
use rustix::event::{PollFd, PollFlags, Timespec};

/// How often the connection thread looks for new requests while it waits for replies.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How long a lost connection waits before it connects again.
const RECONNECT_AFTER: Duration = Duration::from_secs(3);

/// Makes the connection; called once at the start and again after the connection is lost.
pub(crate) type Connector = Box<dyn FnMut() -> Result<Client, InferError> + Send>;
/// Takes a request's reply once it came, or reports the lost connection when it gets `None`.
/// Returns `true` when the request is finished.
pub(crate) type Poller = Box<dyn FnMut(Option<&mut Client>) -> bool + Send>;
/// Sends a request on the connection, or reports that there is none.
pub(crate) type Begin = Box<dyn FnOnce(Option<&mut Client>) -> Option<Poller> + Send>;

pub(crate) enum Command {
    Begin(Begin),
    Cancel(u64),
}

/// The handle of the connection thread. The thread ends when the last handle is dropped.
#[derive(Debug, Clone)]
pub(crate) struct SharedConnection {
    commands: Sender<Command>,
}

impl SharedConnection {
    /// Connects now, on the caller's thread, then serves the connection on its own thread.
    pub(crate) fn open(mut connector: Connector) -> Self {
        let first = connector()
            .map_err(|error| tracing::warn!(%error, "no inference connection yet"))
            .ok();
        let (commands, received) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("context-inferd".to_owned())
            .spawn(move || serve(first, connector, &received));
        if let Err(error) = spawned {
            tracing::error!(%error, "cannot start the inference connection thread");
        }
        Self { commands }
    }

    pub(crate) fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }
}

fn serve(mut client: Option<Client>, mut connector: Connector, commands: &Receiver<Command>) {
    let mut pollers: Vec<Poller> = Vec::new();
    let mut last_attempt = Instant::now();
    loop {
        let first = if !pollers.is_empty() {
            match commands.try_recv() {
                Ok(command) => Some(command),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        } else if client.is_some() {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        } else {
            let wait = RECONNECT_AFTER.saturating_sub(last_attempt.elapsed());
            match commands.recv_timeout(wait.max(Duration::from_millis(1))) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        for command in first.into_iter().chain(commands.try_iter()) {
            run(command, client.as_mut(), &mut pollers);
        }
        let Some(connected) = client.as_mut() else {
            if last_attempt.elapsed() >= RECONNECT_AFTER {
                last_attempt = Instant::now();
                client = connector()
                    .map_err(|error| tracing::debug!(%error, "inferd is still unreachable"))
                    .ok();
            }
            continue;
        };
        let alive = exchange(connected);
        pollers.retain_mut(|poller| !poller(Some(&mut *connected)));
        if !alive {
            tracing::warn!("the inference connection closed; connecting again");
            for mut poller in pollers.drain(..) {
                poller(None);
            }
            client = None;
            last_attempt = Instant::now()
                .checked_sub(RECONNECT_AFTER)
                .unwrap_or_else(Instant::now);
        }
    }
}

fn run(command: Command, client: Option<&mut Client>, pollers: &mut Vec<Poller>) {
    match command {
        Command::Begin(begin) => pollers.extend(begin(client)),
        Command::Cancel(job_id) => {
            if let Some(client) = client
                && let Ok(pending) = client.cancel(job_id)
            {
                pollers.push(Box::new(move |client| {
                    client.is_none_or(|client| !matches!(client.take(&pending), Ok(None)))
                }));
            }
        }
    }
}

/// Sends what is queued and reads what came, waiting up to [`POLL_INTERVAL`]. Returns whether
/// the connection still works.
fn exchange(client: &mut Client) -> bool {
    let readable = {
        let socket = client.as_fd();
        let mut descriptors = [PollFd::new(&socket, PollFlags::IN)];
        let timeout = Timespec::try_from(POLL_INTERVAL).ok();
        rustix::event::poll(&mut descriptors, timeout.as_ref()).is_ok_and(|ready| ready > 0)
    };
    if client.handle_writable().is_err() {
        return false;
    }
    let read = !readable || client.handle_readable().is_ok();
    read && !client.is_closed()
}
