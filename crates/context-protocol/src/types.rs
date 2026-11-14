use serde::{Deserialize, Serialize};

pub use context_core::{
    ActivityLabel, DocKey, EpisodeId, EpisodeLevel, MemoryKind, MemoryRef, Micros, SegmentId,
    TimeRange, Timestamp,
};
use context_core::{AppRef, Episode, InputProfile, Segment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureState {
    Observing,
    Paused,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub state: CaptureState,
    pub pause_reason: Option<String>,
    /// Whether the compositor's context global is bound.
    pub compositor_connected: bool,
    pub observations_seen: u64,
    pub observations_dropped: u64,
    pub open_segments: u32,
    pub pending_writes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FocusView {
    pub app: AppRef,
    pub title: String,
    pub doc_key: String,
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityView {
    pub label: ActivityLabel,
    pub confidence: f32,
    pub since: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeGuess {
    pub id: EpisodeId,
    pub title: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentContext {
    pub t: Timestamp,
    pub focus: Option<FocusView>,
    pub activity: Option<ActivityView>,
    pub episode: Option<EpisodeGuess>,
    /// Document keys touched recently, most recent first.
    pub recent_documents: Vec<String>,
    pub open_documents: u32,
    pub observing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentView {
    pub id: SegmentId,
    pub episode: Option<EpisodeId>,
    pub app_key: String,
    pub doc_key: String,
    pub title: String,
    pub url: Option<String>,
    pub range: TimeRange,
    pub active: Micros,
    pub input: InputProfile,
    pub label: ActivityLabel,
    pub label_confidence: f32,
    pub importance: f32,
    pub summary: Option<String>,
}

impl From<&Segment> for SegmentView {
    fn from(segment: &Segment) -> Self {
        Self {
            id: segment.id,
            episode: segment.episode,
            app_key: segment.app_key.clone(),
            doc_key: segment.doc_key.to_string(),
            title: segment.title.clone(),
            url: segment.url.clone(),
            range: segment.range,
            active: segment.active,
            input: segment.input,
            label: segment.label,
            label_confidence: segment.label_confidence,
            importance: segment.importance,
            summary: segment.summary.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeView {
    pub id: EpisodeId,
    pub level: EpisodeLevel,
    pub range: TimeRange,
    pub title: String,
    pub summary: Option<String>,
    pub label: ActivityLabel,
    pub importance: f32,
    pub confidence: f32,
    pub segments: Vec<SegmentView>,
}

impl EpisodeView {
    pub fn new(episode: &Episode, segments: Vec<SegmentView>) -> Self {
        Self {
            id: episode.id,
            level: episode.level,
            range: episode.range,
            title: episode.title.clone(),
            summary: episode.summary.clone(),
            label: episode.label,
            importance: episode.importance,
            confidence: episode.confidence,
            segments,
        }
    }
}

/// A time range by name or by bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeSelector {
    Today,
    Yesterday,
    LastMinutes(u32),
    LastHours(u32),
    LastDays(u32),
    Between(TimeRange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Redaction {
    SummaryOnly,
    Snippets,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultKind {
    Segment,
    Episode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub time: Option<TimeSelector>,
    pub app_key: Option<String>,
    pub kinds: Vec<ResultKind>,
    pub limit: u32,
    pub redaction: Redaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    CrownUi,
    Accessibility,
    Ocr,
    Vision,
    Llm,
    Compositor,
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: SourceKind,
    pub source_id: String,
    pub t: Timestamp,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub kind: ResultKind,
    pub id: u64,
    pub app_key: String,
    pub title: String,
    pub doc_key: String,
    pub t_start: Timestamp,
    pub episode: Option<EpisodeId>,
    pub snippet: Option<String>,
    pub score: f32,
    pub confidence: f32,
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub took_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Granularity {
    Segment,
    Episode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineQuery {
    pub time: TimeSelector,
    pub granularity: Granularity,
    pub min_importance: f32,
    pub app_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub range: TimeRange,
    pub day_summary: Option<String>,
    pub segments: Vec<SegmentView>,
    pub episodes: Vec<EpisodeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub memory: MemoryRef,
    pub evidence: Vec<Provenance>,
    pub derived_from: Vec<MemoryRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssertionSource {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAssertion {
    pub kind: MemoryKind,
    pub project: Option<String>,
    pub key: String,
    pub value: String,
    pub source: AssertionSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetRequest {
    pub time: TimeSelector,
    pub app_key: Option<String>,
    pub cascade: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ForgetReport {
    pub events: u64,
    pub segments: u64,
    pub text_lines: u64,
    pub frames: u64,
    pub episodes_removed: u64,
}

impl TimeSelector {
    /// The bounds the selector names at `now`, with days starting at local midnight approximated
    /// as UTC midnight shifted by `tz_offset`.
    pub fn resolve(self, now: Timestamp, tz_offset: Micros) -> TimeRange {
        const DAY: i64 = 86_400 * 1_000_000;
        let local_midnight = |t: Timestamp| {
            let local = t.0 + tz_offset.0;
            Timestamp(local - local.rem_euclid(DAY) - tz_offset.0)
        };
        match self {
            Self::Today => TimeRange::new(local_midnight(now), now),
            Self::Yesterday => {
                let today = local_midnight(now);
                TimeRange::new(Timestamp(today.0 - DAY), today)
            }
            Self::LastMinutes(minutes) => {
                TimeRange::new(Timestamp(now.0 - i64::from(minutes) * 60_000_000), now)
            }
            Self::LastHours(hours) => {
                TimeRange::new(Timestamp(now.0 - i64::from(hours) * 3_600_000_000), now)
            }
            Self::LastDays(days) => TimeRange::new(Timestamp(now.0 - i64::from(days) * DAY), now),
            Self::Between(range) => range,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn yesterday_ends_where_today_begins() {
        let now = Timestamp::from_secs(3 * 86_400 + 5_000);
        let today = TimeSelector::Today.resolve(now, Micros::ZERO);
        let yesterday = TimeSelector::Yesterday.resolve(now, Micros::ZERO);
        assert_eq!(today.start, yesterday.end);
        assert_eq!(today.start, Timestamp::from_secs(3 * 86_400));
        assert_eq!(yesterday.duration(), Micros::from_secs(86_400));
    }

    #[test]
    fn a_timezone_moves_midnight() {
        let now = Timestamp::from_secs(86_400 + 3_600);
        let offset = Micros::from_secs(2 * 3_600);
        let today = TimeSelector::Today.resolve(now, offset);
        assert_eq!(today.start, Timestamp::from_secs(86_400 - 2 * 3_600));
    }

    #[test]
    fn views_carry_the_segment_through() {
        let segment = Segment {
            id: SegmentId(1),
            episode: None,
            app_key: "code-oss".into(),
            doc_key: DocKey::File("/src/main.rs".into()),
            title: "main.rs".into(),
            url: None,
            file_path: Some("/src/main.rs".into()),
            cwd: None,
            project: None,
            range: TimeRange::new(Timestamp(0), Timestamp(10)),
            active: Micros(10),
            input: InputProfile::default(),
            label: ActivityLabel::Coding,
            label_confidence: 0.9,
            importance: 0.5,
            text_coverage: 0.0,
            summary: None,
            state: context_core::SegmentState::Closed,
        };
        let view = SegmentView::from(&segment);
        assert_eq!(view.doc_key, "file:/src/main.rs");
        let bytes = postcard::to_stdvec(&view).unwrap();
        let back: SegmentView = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, view);
    }
}
