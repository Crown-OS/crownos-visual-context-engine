use context_core::Observation;
use serde::{Deserialize, Serialize};

use crate::gold::GoldSegment;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub seed: u64,
    pub observations: Vec<Observation>,
    pub gold: Vec<GoldSegment>,
}

impl Trace {
    /// # Errors
    ///
    /// Fails when the JSON is not a trace.
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text)
    }

    /// # Errors
    ///
    /// Fails only when a float is not finite, which the generator never produces.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}
