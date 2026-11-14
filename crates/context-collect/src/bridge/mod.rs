//! The compositor bridge: a wayland-client on its own thread, bound to `crownos_context_v1`,
//! turning events into observations. The global may not exist yet when the engine starts, so the
//! thread waits for it to appear and reconnects if the compositor goes away.

mod dispatch;
mod state;

use std::sync::mpsc::SyncSender;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use context_core::Observation;
use wayland_client::globals::{BindError, registry_queue_init};
use wayland_client::{Connection, DispatchError};

use state::BridgeState;

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
}

const RETRY: Duration = Duration::from_secs(3);

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
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
pub struct CompositorBridge;

impl CompositorBridge {
    /// # Errors
    ///
    /// Fails only when the thread cannot be spawned; connection problems are retried inside.
    pub fn spawn(outlet: Outlet) -> Result<BridgeHandle, BridgeError> {
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("context-bridge".to_owned())
            .spawn({
                let stop = Arc::clone(&stop);
                let connected = Arc::clone(&connected);
                move || run_forever(&outlet, &stop, &connected)
            })?;
        Ok(BridgeHandle {
            stop,
            connected,
            thread: Some(thread),
        })
    }
}

fn run_forever(outlet: &Outlet, stop: &AtomicBool, connected: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        match run_once(outlet, stop, connected) {
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

fn run_once(outlet: &Outlet, stop: &AtomicBool, connected: &AtomicBool) -> Result<(), BridgeError> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<BridgeState>(&connection)?;
    let handle = queue.handle();
    let mut state = BridgeState::bind(&globals, &handle, outlet.clone())?;
    connected.store(true, Ordering::Relaxed);
    tracing::info!("bound crownos_context_manager_v1");
    while !stop.load(Ordering::Relaxed) {
        queue.blocking_dispatch(&mut state)?;
        if state.outlet_closed() {
            break;
        }
    }
    state.manager.destroy();
    let _ = queue.flush();
    Ok(())
}
