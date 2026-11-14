//! How much a segment deserves to be recalled, 0 to 1.

use context_core::{Micros, Segment};

const ACTIVE_WEIGHT: f32 = 0.45;
const TYPING_WEIGHT: f32 = 0.25;
const ACTIONS_WEIGHT: f32 = 0.15;
const PROJECT_WEIGHT: f32 = 0.15;
/// Active minutes at which the time term saturates.
const SATURATION_MINUTES: f32 = 45.0;
const TYPING_SATURATION: f32 = 120.0;

/// `copies` is how many clipboard copies happened in the segment.
pub fn importance(segment: &Segment, copies: u32) -> f32 {
    let minutes = minutes(segment.active);
    let time_term = (1.0 + minutes).ln() / (1.0 + SATURATION_MINUTES).ln();
    let typing_term = (segment.input.typing_rate(segment.active) / TYPING_SATURATION).min(1.0);
    let actions_term = (copies as f32 / 5.0).min(1.0);
    let project_term = f32::from(segment.project.is_some());
    (ACTIVE_WEIGHT * time_term.min(1.0)
        + TYPING_WEIGHT * typing_term
        + ACTIONS_WEIGHT * actions_term
        + PROJECT_WEIGHT * project_term)
        .clamp(0.0, 1.0)
}

fn minutes(active: Micros) -> f32 {
    active.as_secs() as f32 / 60.0
}

#[cfg(test)]
mod tests {
    use context_core::{
        ActivityLabel, DocKey, InputProfile, SegmentId, SegmentState, TimeRange, Timestamp,
    };

    use super::*;

    fn segment(active_secs: i64, text_keys: u32) -> Segment {
        Segment {
            id: SegmentId(1),
            episode: None,
            app_key: "a".into(),
            doc_key: DocKey::Title("t".into()),
            title: String::new(),
            url: None,
            file_path: None,
            cwd: None,
            project: None,
            range: TimeRange::new(Timestamp(0), Timestamp::from_secs(active_secs)),
            active: Micros::from_secs(active_secs),
            input: InputProfile {
                text_keys,
                ..InputProfile::default()
            },
            label: ActivityLabel::Other,
            label_confidence: 0.0,
            importance: 0.0,
            text_coverage: 0.0,
            summary: None,
            state: SegmentState::Closed,
        }
    }

    #[test]
    fn longer_busier_segments_matter_more_within_bounds() {
        let glance = importance(&segment(5, 0), 0);
        let hour = importance(&segment(3_600, 6_000), 3);
        assert!(glance < hour);
        assert!(glance >= 0.0 && hour <= 1.0);
        assert!(importance(&segment(3_600, 0), 0) < hour);
    }
}
