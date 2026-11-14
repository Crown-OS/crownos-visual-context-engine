//! The engine against the synthetic day: boundaries it should find, invariants it must keep.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use context_core::{Micros, ObservationKind, Segment, SessionState, Timestamp};
use context_engine::{Engine, EngineEvent, SegmentEvent};
use context_eval::{Scenario, Trace, generate};

const TOLERANCE: Micros = Micros::from_secs(2);

fn closed_segments(trace: &Trace) -> Vec<Segment> {
    let mut engine = Engine::new(Some("/home/u".to_owned()));
    let mut closed = Vec::new();
    for observation in &trace.observations {
        for event in engine.ingest(observation) {
            if let EngineEvent::Segment(SegmentEvent::Closed(segment)) = event {
                closed.push(segment);
            }
        }
    }
    closed
}

const fn near(a: Timestamp, b: Timestamp) -> bool {
    a.since(b).0 <= TOLERANCE.0 && b.since(a).0 <= TOLERANCE.0
}

/// Precision and recall of segment boundaries against the gold ones, within tolerance.
fn boundary_f1(found: &[Segment], trace: &Trace) -> f64 {
    let gold: Vec<_> = trace
        .gold
        .iter()
        .map(|g| (g.range.start, g.range.end))
        .collect();
    let hits = found
        .iter()
        .filter(|segment| {
            gold.iter().any(|(start, end)| {
                near(segment.range.start, *start) && near(segment.range.end, *end)
            })
        })
        .count() as f64;
    let precision = hits / found.len().max(1) as f64;
    let recall = hits / gold.len().max(1) as f64;
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

#[test]
fn the_workday_is_segmented_like_the_gold() {
    let trace = generate(Scenario::Workday, 1);
    let found = closed_segments(&trace);
    let f1 = boundary_f1(&found, &trace);
    assert!(
        f1 >= 0.9,
        "boundary F1 {f1:.3} with {} found vs {} gold",
        found.len(),
        trace.gold.len()
    );
}

#[test]
fn labels_follow_the_apps() {
    let trace = generate(Scenario::Short, 3);
    let found = closed_segments(&trace);
    let mut by_app: BTreeMap<&str, Vec<&Segment>> = BTreeMap::new();
    for segment in &found {
        by_app
            .entry(segment.app_key.as_str())
            .or_default()
            .push(segment);
    }
    let coding = by_app.get("code-oss").unwrap();
    assert!(
        coding
            .iter()
            .all(|s| s.label == context_core::ActivityLabel::Coding)
    );
    let browsing = by_app.get("firefox").unwrap();
    assert!(browsing.iter().all(|s| matches!(
        s.label,
        context_core::ActivityLabel::ResearchBrowsing
            | context_core::ActivityLabel::ReadingDocs
            | context_core::ActivityLabel::Coding
    )));
}

#[test]
fn segments_never_overlap_and_active_time_fits_the_day() {
    for seed in 1..=12 {
        let trace = generate(Scenario::Workday, seed);
        let found = closed_segments(&trace);
        for pair in found.windows(2) {
            assert!(
                pair[0].range.end.0 <= pair[1].range.start.0,
                "seed {seed}: overlap"
            );
        }
        let wall = trace
            .observations
            .last()
            .unwrap()
            .t
            .since(trace.observations[0].t);
        let active: i64 = found.iter().map(|s| s.active.0).sum();
        assert!(
            active <= wall.0,
            "seed {seed}: active {active} > wall {}",
            wall.0
        );
        for segment in &found {
            assert!(segment.active.0 <= segment.range.duration().0 + Micros::from_secs(10).0);
            assert!(segment.importance >= 0.0 && segment.importance <= 1.0);
        }
    }
}

#[test]
fn a_lock_closes_the_open_segment() {
    let trace = generate(Scenario::Workday, 2);
    let lock_at = trace
        .observations
        .iter()
        .find(|o| matches!(o.kind, ObservationKind::Session(SessionState::Locked)))
        .unwrap()
        .t;
    let found = closed_segments(&trace);
    assert!(
        found
            .iter()
            .all(|s| s.range.end.0 <= lock_at.0 || s.range.start.0 >= lock_at.0)
    );
}
