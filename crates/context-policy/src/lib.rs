//! What the engine may look at. Every observation and every derived artifact passes the
//! [`Policy`] gate; unknown state fails closed. The rules are the `visual_context` section of
//! crownos-config, which the compositor reads too, so its capture veto holds even when the
//! engine is not running.

mod gate;
mod secrets;
mod util;

pub use crownos_config::schema::VisualContext;
pub use gate::{Policy, PolicyError, Reason, Verdict};
pub use secrets::{SecretKind, SecretMatch, find_secrets};
