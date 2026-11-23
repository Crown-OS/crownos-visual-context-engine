use serde::{Deserialize, Serialize};

use crate::registry::Capability;

/// A job a caller asks for. The registry maps each one to ranked model candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Task {
    RecognizeText,
    EmbedText,
    Rerank,
    SummarizeSegments,
    DescribeScreen,
    Transcribe,
}

impl Task {
    /// Whether a call costs about the same each time, so a measured latency predicts the next
    /// one. The cost of generation grows with the requested tokens.
    pub const fn has_fixed_cost(self) -> bool {
        matches!(self, Self::RecognizeText | Self::EmbedText | Self::Rerank)
    }

    pub const fn required_capabilities(self) -> &'static [Capability] {
        match self {
            Self::RecognizeText => &[Capability::Ocr],
            Self::EmbedText => &[Capability::Embed],
            Self::Rerank => &[Capability::Rerank],
            Self::SummarizeSegments => &[Capability::Generate, Capability::JsonOutput],
            Self::DescribeScreen => &[
                Capability::Generate,
                Capability::Vision,
                Capability::JsonOutput,
            ],
            Self::Transcribe => &[Capability::Transcribe],
        }
    }
}
