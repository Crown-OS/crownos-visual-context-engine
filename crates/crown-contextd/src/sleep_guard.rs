//! `mlock` does not keep pages out of a hibernation image, so every frame is zeroed before the
//! system sleeps. A logind delay inhibitor holds the sleep back until that is done.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use context_capture::{CaptureHold, CaptureService};
use futures_util::StreamExt;
use zbus::zvariant::OwnedFd;

/// Shorter than logind's default `InhibitDelayMaxSec` of 5 s.
const ZERO_FRAMES_TIMEOUT: Duration = Duration::from_secs(4);

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
pub(crate) async fn guard_sleep(capture: Arc<CaptureService>) -> Result<()> {
    let connection = zbus::Connection::system()
        .await
        .context("connecting to the system bus")?;
    let manager = LoginManagerProxy::new(&connection).await?;
    let mut sleeps = manager.receive_prepare_for_sleep().await?;
    let mut inhibitor = Some(take_inhibitor(&manager).await?);
    while let Some(signal) = sleeps.next().await {
        if signal.args()?.start {
            let capture = Arc::clone(&capture);
            let zeroed = tokio::task::spawn_blocking(move || {
                capture.suspend(CaptureHold::Sleep, ZERO_FRAMES_TIMEOUT)
            })
            .await
            .unwrap_or(false);
            if zeroed {
                tracing::info!("captures stopped and frames zeroed before sleep");
            } else {
                tracing::warn!(
                    "the running analysis did not end before the inhibitor delay; captures are \
                     held, but a frame may still be in memory"
                );
            }
            drop(inhibitor.take());
        } else {
            capture.resume(CaptureHold::Sleep);
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
            "crown-contextd",
            "Zero captured frames before sleep",
            "delay",
        )
        .await
        .context("taking a logind delay inhibitor")
}
