use serde::{Deserialize, Serialize};

use crate::ids::{EpisodeId, ProjectId, SegmentId};
use crate::observation::InputCounts;
use crate::time::{Micros, TimeRange};

/// What a segment is about: the first of these that is known, in this order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DocKey {
    Document(String),
    Url(String),
    File(String),
    Directory(String),
    Title(String),
}

impl DocKey {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Document(key)
            | Self::Url(key)
            | Self::File(key)
            | Self::Directory(key)
            | Self::Title(key) => key,
        }
    }

    pub const fn scheme(&self) -> &'static str {
        match self {
            Self::Document(_) => "doc",
            Self::Url(_) => "url",
            Self::File(_) => "file",
            Self::Directory(_) => "dir",
            Self::Title(_) => "title",
        }
    }
}

impl std::fmt::Display for DocKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.scheme(), self.as_str())
    }
}

/// Taxonomy v1 of what the user is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ActivityLabel {
    Coding,
    CodeReview,
    TerminalOps,
    ReadingDocs,
    ResearchBrowsing,
    Writing,
    Spreadsheet,
    Design,
    CommunicationChat,
    Email,
    Meeting,
    MediaConsumption,
    Gaming,
    AdminSettings,
    FileManagement,
    Shopping,
    Finance,
    Learning,
    #[default]
    Other,
}

impl ActivityLabel {
    pub const ALL: [Self; 19] = [
        Self::Coding,
        Self::CodeReview,
        Self::TerminalOps,
        Self::ReadingDocs,
        Self::ResearchBrowsing,
        Self::Writing,
        Self::Spreadsheet,
        Self::Design,
        Self::CommunicationChat,
        Self::Email,
        Self::Meeting,
        Self::MediaConsumption,
        Self::Gaming,
        Self::AdminSettings,
        Self::FileManagement,
        Self::Shopping,
        Self::Finance,
        Self::Learning,
        Self::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::CodeReview => "code_review",
            Self::TerminalOps => "terminal_ops",
            Self::ReadingDocs => "reading_docs",
            Self::ResearchBrowsing => "research_browsing",
            Self::Writing => "writing",
            Self::Spreadsheet => "spreadsheet",
            Self::Design => "design",
            Self::CommunicationChat => "communication_chat",
            Self::Email => "email",
            Self::Meeting => "meeting",
            Self::MediaConsumption => "media_consumption",
            Self::Gaming => "gaming",
            Self::AdminSettings => "admin_settings",
            Self::FileManagement => "file_management",
            Self::Shopping => "shopping",
            Self::Finance => "finance",
            Self::Learning => "learning",
            Self::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|label| label.as_str() == text)
    }
}

/// The input a segment saw, summed over its intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InputProfile {
    pub text_keys: u32,
    pub edit_keys: u32,
    pub nav_keys: u32,
    pub chords: u32,
    pub clicks: u32,
    pub scroll_ticks: u32,
    pub pointer_travel: u32,
}

impl InputProfile {
    pub const fn add(&mut self, counts: &InputCounts) {
        self.text_keys = self.text_keys.saturating_add(counts.text_keys);
        self.edit_keys = self.edit_keys.saturating_add(counts.edit_keys);
        self.nav_keys = self.nav_keys.saturating_add(counts.nav_keys);
        self.chords = self.chords.saturating_add(counts.chords);
        self.clicks = self.clicks.saturating_add(counts.clicks);
        self.scroll_ticks = self.scroll_ticks.saturating_add(counts.scroll_ticks);
        self.pointer_travel = self.pointer_travel.saturating_add(counts.pointer_travel);
    }

    /// Keys per minute of active time, the one number the classifier and importance use.
    pub fn typing_rate(&self, active: Micros) -> f32 {
        let minutes = active.as_secs() as f32 / 60.0;
        if minutes <= 0.0 {
            return 0.0;
        }
        (self.text_keys + self.edit_keys) as f32 / minutes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SegmentState {
    #[default]
    Open,
    Closed,
    Summarized,
}

/// Time on one document in one app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub episode: Option<EpisodeId>,
    pub app_key: String,
    pub doc_key: DocKey,
    pub title: String,
    pub url: Option<String>,
    pub file_path: Option<String>,
    pub cwd: Option<String>,
    pub project: Option<ProjectId>,
    pub range: TimeRange,
    pub active: Micros,
    pub input: InputProfile,
    pub label: ActivityLabel,
    pub label_confidence: f32,
    pub importance: f32,
    pub text_coverage: f32,
    pub summary: Option<String>,
    pub state: SegmentState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EpisodeLevel {
    #[default]
    Task,
    Day,
    Week,
}

/// Segments that belong together: one task, or a day's or week's rollup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: EpisodeId,
    pub level: EpisodeLevel,
    pub parent: Option<EpisodeId>,
    pub range: TimeRange,
    pub title: String,
    pub summary: Option<String>,
    pub label: ActivityLabel,
    pub project: Option<ProjectId>,
    pub importance: f32,
    pub confidence: f32,
    pub segments: Vec<SegmentId>,
}
