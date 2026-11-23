//! The compositor bridge: a wayland-client on its own thread, bound to `crownos_context_v1`,
//! turning events into observations. The global may not exist yet when the engine starts, so the
//! thread waits for it to appear and reconnects if the compositor goes away.

mod dispatch;
mod link;
mod state;

use std::sync::mpsc::SyncSender;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use context_core::Observation;
use rustix::event::{PollFd, PollFlags, Timespec};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{BindError, registry_queue_init};
use wayland_client::{Connection, DispatchError, EventQueue, Proxy};

pub use crate::util::identifiable_connect::ExposureRefused;
use crownos_protocols::context::v1::client::crownos_context_manager_v1::Scope;
pub use link::{CompositorLink, ExposureHook, SensitiveReason};
use state::BridgeState;

use crate::util::identifiable_connect::connect_identifiable;

/// How long a stop waits for the compositor bridge thread.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// Where the bridge puts what it sees.
pub type Outlet = SyncSender<Observation>;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("cannot connect to the compositor: {0}")]
    Connect(#[from] wayland_client::ConnectError),
    #[error("the registry did not answer: {0}")]
    Registry(#[from] wayland_client::globals::GlobalError),
    #[error("crownos_context_manager_v1 is not offered: {0}")]
    Bind(#[from] BindError),
    #[error("the compositor connection failed: {0}")]
    Dispatch(#[from] DispatchError),
    #[error("cannot start the bridge thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error(transparent)]
    ExposureRefused(#[from] ExposureRefused),
    #[error("the compositor withdrew the grant of this engine")]
    Revoked,
}

const RETRY: Duration = Duration::from_secs(3);
/// How long the bridge waits for compositor events before it looks at its stop flag.
const STOP_CHECK: Duration = Duration::from_millis(200);

/// Spawns the bridge thread. Dropping the handle asks the thread to stop.
#[derive(Debug)]
pub struct BridgeHandle {
    stop: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl BridgeHandle {
    /// Whether the context global is bound right now.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && !context_core::join_within(thread, STOP_WAIT)
        {
            tracing::warn!(waited = ?STOP_WAIT, "the compositor bridge thread did not stop; leaving it");
        }
    }
}

#[derive(Debug)]
pub struct CompositorBridge;

impl CompositorBridge {
    /// # Errors
    ///
    /// Fails only when the thread cannot be spawned; connection problems are retried inside.
    pub fn spawn(outlet: Outlet, link: CompositorLink) -> Result<BridgeHandle, BridgeError> {
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("context-bridge".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                let connected = Arc::clone(&connected);
                move || run_forever(&outlet, &link, &stop, &connected)
            })?;
        Ok(BridgeHandle {
            stop,
            connected,
            thread: Some(thread),
        })
    }
}

fn run_forever(outlet: &Outlet, link: &CompositorLink, stop: &AtomicBool, connected: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let outcome = run_once(outlet, link, stop, connected);
        link.detach();
        match outcome {
            Ok(()) => return,
            Err(BridgeError::Bind(BindError::NotPresent)) => {
                tracing::info!("the compositor offers no context protocol yet; waiting")
            }
            Err(error) => tracing::warn!(%error, "compositor bridge stopped; retrying"),
        }
        connected.store(false, Ordering::Relaxed);
        std::thread::sleep(RETRY);
    }
}

fn run_once(
    outlet: &Outlet,
    link: &CompositorLink,
    stop: &AtomicBool,
    connected: &AtomicBool,
) -> Result<(), BridgeError> {
    let exposure = link.exposure_hook();
    let (connection, globals, mut queue) = connect_identifiable(exposure.as_ref(), || {
        let connection = Connection::connect_to_env()?;
        let (globals, queue) = registry_queue_init::<BridgeState>(&connection)?;
        Ok::<_, BridgeError>((connection, globals, queue))
    })?;
    let handle = queue.handle();
    let mut state = BridgeState::bind(&globals, &handle, outlet.clone(), link.clone())?;
    if state.manager.version() < 2 {
        link.set_granted(Scope::Engine.bits());
    }
    link.attach(connection);
    connected.store(true, Ordering::Relaxed);
    tracing::info!("bound crownos_context_manager_v1");
    while !stop.load(Ordering::Relaxed) {
        dispatch_within(&mut queue, &mut state, STOP_CHECK)?;
        if state.is_revoked() {
            return Err(BridgeError::Revoked);
        }
        if state.outlet_closed() {
            break;
        }
    }
    state.manager.destroy();
    let _ = queue.flush();
    Ok(())
}

/// Dispatches the events that come within `timeout`. A blocking dispatch would wait for the
/// next compositor event, which may never come, and so hold back a stop.
fn dispatch_within(
    queue: &mut EventQueue<BridgeState>,
    state: &mut BridgeState,
    timeout: Duration,
) -> Result<(), BridgeError> {
    queue.flush().map_err(DispatchError::from)?;
    queue.dispatch_pending(state)?;
    if let Some(guard) = queue.prepare_read() {
        let socket = guard.connection_fd();
        let mut descriptors = [PollFd::new(&socket, PollFlags::IN)];
        let timeout = Timespec::try_from(timeout).ok();
        let readable =
            rustix::event::poll(&mut descriptors, timeout.as_ref()).is_ok_and(|ready| ready > 0);
        if readable {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(DispatchError::from(error).into()),
            }
        }
    }
    queue.dispatch_pending(state)?;
    Ok(())
}
