use context_core::{ActivityLabel, TimeRange};
use serde::{Deserialize, Serialize};

/// What the segmenter should find: one stretch of attention on one document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldSegment {
    pub app_id: String,
    pub window: String,
    pub doc_key: String,
    pub range: TimeRange,
    pub label: ActivityLabel,
}
