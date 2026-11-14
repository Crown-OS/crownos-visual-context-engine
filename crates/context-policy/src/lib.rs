//! What the engine may look at. Every observation and every derived artifact passes the
//! [`Policy`] gate; unknown state fails closed. The rules come from
//! `~/.config/crownos/visual_context.ron`, and the compositor reads the same file so its capture
//! veto holds even when the engine is not running.

mod config;
mod gate;
mod secrets;
mod util;

pub use config::{VisualContext, VisualContextKeys};
pub use gate::{Policy, PolicyError, Reason, Verdict};
pub use secrets::{SecretKind, SecretMatch, find_secrets};
