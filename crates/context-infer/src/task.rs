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
    /// Whether the time per unit of work stays about the same, so a measured latency
    /// predicts the next call. Generation time depends on the tokens the model makes.
    pub const fn has_predictable_cost(self) -> bool {
        matches!(
            self,
            Self::RecognizeText | Self::EmbedText | Self::Rerank | Self::Transcribe
        )
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
