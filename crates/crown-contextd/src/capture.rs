//! The capture pipeline as the daemon runs it: which capture may happen, and where its text goes.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use anyhow::{Context, Result};
use context_capture::{
    CaptureHold, CaptureOutcome, CaptureService, CaptureServiceParts, CaptureTarget,
    InferScreenModels, NoStructuredText,
};
use context_collect::{BridgeLink, SensitiveReason};
use context_core::{Observation, Trigger, WindowIdent};
use context_infer_client::InferdClient;
use context_policy::{Policy, Verdict};

use crate::local_sensitivity::LocalSensitivity;
use crate::shared::Shared;
use crate::util::power_supply::on_battery;

/// How long the bridge waits for the frames to be zeroed before it shows the process to the
/// compositor.
const IDENTITY_EXPOSURE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the compositor refuses captures of a window that showed a secret.
pub(crate) const SECRET_FLAG_DURATION: Duration = Duration::from_secs(10 * 60);

/// Starts capture and analysis. Outcomes come back as observations through `outlet`, so they
/// pass the same policy gate as everything else.
/// Models run in crown-context-inferd, found in `socket_dir` (default
/// `$XDG_RUNTIME_DIR/crownos`).
pub(crate) fn spawn(
    link: &BridgeLink,
    outlet: SyncSender<Observation>,
    socket_dir: Option<PathBuf>,
) -> Result<Arc<CaptureService>> {
    let deliver = move |outcome: CaptureOutcome| {
        for observation in outcome.observations() {
            if outlet.send(observation).is_err() {
                return;
            }
        }
    };
    let parts = CaptureServiceParts {
        models: Arc::new(InferScreenModels::new(InferdClient::new(socket_dir))),
        structured_text: Box::new(NoStructuredText),
        on_battery: Box::new(on_battery),
        deliver: Box::new(deliver),
    };
    let service = Arc::new(
        CaptureService::spawn(link.clone(), parts).context("starting the capture pipeline")?,
    );
    let held = Arc::downgrade(&service);
    link.set_exposure_hook(Arc::new(move |exposed| {
        let Some(service) = held.upgrade() else {
            return true;
        };
        if exposed {
            service.suspend(CaptureHold::IdentityExposed, IDENTITY_EXPOSURE_TIMEOUT)
        } else {
            service.resume(CaptureHold::IdentityExposed);
            true
        }
    }));
    Ok(service)
}

/// Asks the compositor to refuse captures of `window` for a while. Returns whether the request
/// went out.
pub(crate) fn flag_secret(link: &BridgeLink, window: &WindowIdent) -> bool {
    let flagged = link.flag_sensitive(window, SensitiveReason::SecretText, SECRET_FLAG_DURATION);
    tracing::info!(
        flagged,
        "a secret was on screen: its text is dropped and the window is flagged"
    );
    flagged
}

/// The focused window as a capture target, when every gate passes: observation is on, the
/// window is still the one the trigger was for, the policy allows text from it, and no secret
/// was seen in it lately.
pub(crate) fn target(
    shared: &Shared,
    policy: &Policy,
    locally_sensitive: &LocalSensitivity,
    trigger: Trigger,
    window: Option<&WindowIdent>,
) -> Option<CaptureTarget> {
    if !shared.enabled || shared.paused || !shared.engine.is_observing() {
        return None;
    }
    let focus = shared.engine.working_memory().focus.as_ref()?;
    if window.is_some_and(|window| window != focus) {
        return None;
    }
    let record = shared.engine.window(focus)?;
    if policy.judge(record) != Verdict::Observe || locally_sensitive.is_marked(focus) {
        return None;
    }
    let app = shared.engine.app_of(focus)?;
    Some(CaptureTarget {
        window: focus.clone(),
        trigger,
        app_key: app.app_key,
        app_name: app.name,
        title: record.title.clone(),
    })
}
