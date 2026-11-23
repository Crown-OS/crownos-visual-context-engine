//! Before the system sleeps, no job may hold a frame: `mlock` does not keep pages out of a
//! hibernation image. A logind delay inhibitor holds the sleep back while new jobs are refused
//! and the running ones end; their memfds are then unmapped and closed.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use zbus::zvariant::OwnedFd;

use crate::job_gate::JobGate;

/// Shorter than logind's default `InhibitDelayMaxSec` of 5 s.
const JOBS_END_TIMEOUT: Duration = Duration::from_secs(4);

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
pub(crate) trait LoginManager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;

    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// Runs until the system bus goes away.
pub(crate) async fn guard_sleep(gate: Arc<JobGate>) -> Result<()> {
    let connection = zbus::Connection::system()
        .await
        .context("connecting to the system bus")?;
    let manager = LoginManagerProxy::new(&connection).await?;
    let mut sleeps = manager.receive_prepare_for_sleep().await?;
    let mut inhibitor = Some(take_inhibitor(&manager).await?);
    while let Some(signal) = sleeps.next().await {
        if signal.args()?.start {
            let gate = Arc::clone(&gate);
            let ended = tokio::task::spawn_blocking(move || gate.close_and_wait(JOBS_END_TIMEOUT))
                .await
                .unwrap_or(false);
            if ended {
                tracing::info!("no job holds a frame before sleep");
            } else {
                tracing::warn!(
                    "a job still runs at the inhibitor delay; its frame may be in memory"
                );
            }
            drop(inhibitor.take());
        } else {
            gate.open();
            if inhibitor.is_none() {
                inhibitor = take_inhibitor(&manager)
                    .await
                    .map_err(|error| tracing::warn!(%error, "no sleep inhibitor after resume"))
                    .ok();
            }
        }
    }
    Ok(())
}

async fn take_inhibitor(manager: &LoginManagerProxy<'_>) -> Result<OwnedFd> {
    manager
        .inhibit(
            "sleep",
            "crown-context-inferd",
            "Close frames before sleep",
            "delay",
        )
        .await
        .context("taking a logind delay inhibitor")
}
