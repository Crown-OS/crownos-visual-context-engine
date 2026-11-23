use std::sync::mpsc;
use std::time::Duration;

use rustix::process::{DumpableBehavior, dumpable_behavior, set_dumpable_behavior};

use crate::bridge::ExposureHook;

/// The longest the process stays readable, even when the compositor does not answer.
const LONGEST_EXPOSURE: Duration = Duration::from_secs(3);

/// The compositor and inferd learn which program a client is from `/proc/<pid>/exe` when they
/// accept the connection. A process that turned `PR_SET_DUMPABLE` off hides that link, so it is dumpable
/// again only while `connect` runs, which must include a roundtrip. `exposure` holds the
/// captures first, so no frame exists while the process is readable, and a watchdog ends the
/// exposure after [`LONGEST_EXPOSURE`].
///
/// This is temporary: it goes away when the compositor identifies the processes it spawned.
pub fn connect_identifiable<T, E: From<ExposureRefused>>(
    exposure: Option<&ExposureHook>,
    connect: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let hidden =
        dumpable_behavior().is_ok_and(|behavior| behavior == DumpableBehavior::NotDumpable);
    if !hidden {
        return connect();
    }
    if let Some(exposure) = exposure
        && !exposure(true)
    {
        exposure(false);
        return Err(ExposureRefused.into());
    }
    let (connected, watchdog) = mpsc::channel::<()>();
    let _ = set_dumpable_behavior(DumpableBehavior::Dumpable);
    let watchdog = std::thread::Builder::new()
        .name("context-exposure".to_owned())
        .spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = watchdog.recv_timeout(LONGEST_EXPOSURE) {
                hide();
                tracing::warn!(
                    "the compositor did not answer in time; the process is hidden again"
                );
            }
        });
    let result = connect();
    drop(connected);
    if let Ok(watchdog) = watchdog {
        let _ = watchdog.join();
    }
    hide();
    if let Some(exposure) = exposure {
        exposure(false);
    }
    result
}

/// Frames were still in memory, so the process was not made readable.
#[derive(Debug, thiserror::Error)]
#[error("frames are still in memory; the connection waits")]
pub struct ExposureRefused;

fn hide() {
    if let Err(errno) = set_dumpable_behavior(DumpableBehavior::NotDumpable) {
        tracing::error!(%errno, "cannot hide the process again after connecting");
    }
}
