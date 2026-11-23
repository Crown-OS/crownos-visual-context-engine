//! Reads the text of the focused window from the AT-SPI accessibility bus.
//!
//! The collector listens to a few events only: focus, window activation, document load, name
//! change and text change. When the focused application settles, it fetches the shown subtree of
//! the focused frame by the cheapest strategy the toolkit allows (`Collection.GetMatches`,
//! `Cache.GetItems` or a bounded parallel walk), then the text of the nodes that carry text. A
//! password field stops the read and flags the window sensitive. Each application has a read
//! budget and a backoff; text near to text sent in the last minute is dropped.
//!
//! The compositor turns the session switch `org.a11y.Status.IsEnabled` on at startup. The
//! collector does not touch it.

mod applications;
mod backoff;
mod calls;
mod collector;
mod dedup;
mod events;
mod fetch;
mod health;
mod host;
mod node;
mod read;
mod reader;
mod schedule;
mod toolkit;
mod window_match;

use std::thread::JoinHandle;
use std::time::Duration;

use atspi::AccessibilityConnection;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

pub use calls::AtspiCall;
pub use host::{AccessibilityHost, compositor_allows_read};
pub use read::{FrameContent, WindowReading, read_active_window};
pub use reader::ReadError;
pub use toolkit::{FetchStrategy, Toolkit};

use crate::Outlet;
use collector::Collector;

const RETRY: Duration = Duration::from_secs(10);
const QUEUED_EVENTS: usize = 1_024;

#[derive(Debug, thiserror::Error)]
pub enum A11yError {
    #[error("cannot reach the accessibility bus: {0}")]
    Connect(#[from] atspi::AtspiError),
    #[error("cannot subscribe to accessibility events: {0}")]
    Subscribe(#[from] zbus::Error),
    #[error("the accessibility bus closed")]
    BusClosed,
    #[error("the application shows no frame")]
    NoFrame,
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error("cannot start the accessibility thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// Which a11y bus to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibilityBus {
    /// The one `org.a11y.Bus` on the session bus announces.
    Session,
    /// A bus at this D-Bus address, for tests.
    Address(String),
}

/// Stops the collector thread when dropped.
#[derive(Debug)]
pub struct AccessibilityHandle {
    stop: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for AccessibilityHandle {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
pub struct AccessibilityCollector;

impl AccessibilityCollector {
    /// Starts the collector on its own thread. A missing a11y bus is retried there.
    ///
    /// # Errors
    ///
    /// Fails only when the thread cannot be spawned.
    pub fn spawn<H: AccessibilityHost>(
        outlet: Outlet,
        host: H,
        bus: AccessibilityBus,
    ) -> Result<AccessibilityHandle, A11yError> {
        let (stop, stopped) = watch::channel(false);
        let thread = std::thread::Builder::new()
            .name("context-a11y".to_owned())
            .spawn(move || run_forever(&outlet, &host, &bus, stopped))?;
        Ok(AccessibilityHandle {
            stop,
            thread: Some(thread),
        })
    }
}

fn run_forever<H: AccessibilityHost>(
    outlet: &Outlet,
    host: &H,
    bus: &AccessibilityBus,
    mut stop: watch::Receiver<bool>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!(%error, "cannot start the accessibility runtime");
            return;
        }
    };
    runtime.block_on(async {
        loop {
            let mut session_stop = stop.clone();
            let outcome = tokio::select! {
                outcome = run_once(outlet, host, bus, &mut session_stop) => outcome,
                _ = stop.changed() => return,
            };
            match outcome {
                Ok(()) => return,
                Err(error) => tracing::warn!(%error, "accessibility collector stopped; retrying"),
            }
            tokio::select! {
                () = tokio::time::sleep(RETRY) => {}
                _ = stop.changed() => return,
            }
        }
    });
}

async fn run_once<H: AccessibilityHost>(
    outlet: &Outlet,
    host: &H,
    bus: &AccessibilityBus,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), A11yError> {
    let connection = match bus {
        AccessibilityBus::Session => AccessibilityConnection::new().await?,
        AccessibilityBus::Address(address) => {
            AccessibilityConnection::from_address(address.as_str().try_into()?).await?
        }
    };
    let connection = connection.connection().clone();
    let events = events::subscribe(&connection).await?;
    tracing::info!("reading text through the accessibility bus");
    let (forward, received) = mpsc::channel(QUEUED_EVENTS);
    let forwarder = tokio::spawn(events.for_each(move |event| {
        let _ = forward.try_send(event);
        std::future::ready(())
    }));
    let outcome = Collector::new(connection, host, outlet.clone())
        .run(received, stop)
        .await;
    forwarder.abort();
    outcome
}
