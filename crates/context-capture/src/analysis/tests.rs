#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use context_core::Trigger;

use super::*;
use crate::region_copy::RegionCopy;
use crate::screen_models::{ModelError, RecognizedLine};
use crate::structured_text::NoStructuredText;
use crate::test_frames::{frame_for, is_all_zero, memfd_reader, paint_rect, target};

const AWS_KEY_LINE: &str = "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";

#[derive(Default)]
struct FakeModels {
    ocr_fails: AtomicBool,
    ocr_text: Mutex<String>,
    caption: Mutex<String>,
    ocr_calls: AtomicUsize,
    describe_calls: AtomicUsize,
}

impl FakeModels {
    fn reading(text: &str) -> Arc<Self> {
        let models = Self::default();
        *models.ocr_text.lock().unwrap() = text.to_owned();
        *models.caption.lock().unwrap() = "A photo of a cat".to_owned();
        Arc::new(models)
    }

    fn ocr_calls(&self) -> usize {
        self.ocr_calls.load(Ordering::Relaxed)
    }

    fn describe_calls(&self) -> usize {
        self.describe_calls.load(Ordering::Relaxed)
    }
}

impl ScreenModels for FakeModels {
    fn recognize_text(
        &self,
        region: &RegionCopy,
        _: Instant,
    ) -> Result<Vec<RecognizedLine>, ModelError> {
        self.ocr_calls.fetch_add(1, Ordering::Relaxed);
        if self.ocr_fails.load(Ordering::Relaxed) {
            return Err(ModelError::Failed("busy".to_owned()));
        }
        Ok(vec![RecognizedLine {
            text: self.ocr_text.lock().unwrap().clone(),
            confidence: 0.95,
            bounds: region.bounds(),
        }])
    }

    fn describe_screen(
        &self,
        _: &RegionCopy,
        _: &str,
        _: &str,
        _: Instant,
    ) -> Result<ScreenSummary, ModelError> {
        self.describe_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ScreenSummary {
            content_kind: "image".to_owned(),
            caption: self.caption.lock().unwrap().clone(),
            visible_entities: vec!["Garfield".to_owned()],
        })
    }
}

fn analyzer(models: &Arc<FakeModels>) -> FrameAnalyzer {
    FrameAnalyzer::new(
        Arc::clone(models) as Arc<dyn ScreenModels>,
        Box::new(NoStructuredText),
    )
}

fn line_at(y: u32, damage: Vec<Rect>) -> CapturedFrame {
    frame_for(
        target("editor"),
        200,
        100,
        |pixels| paint_rect(pixels, 200, Rect::new(20, y, 120, 10)),
        damage,
    )
}

fn plugged_in(cancel: &AtomicBool) -> AnalysisConditions<'_> {
    AnalysisConditions {
        on_battery: false,
        cancel,
    }
}

fn on_battery(cancel: &AtomicBool) -> AnalysisConditions<'_> {
    AnalysisConditions {
        on_battery: true,
        cancel,
    }
}

#[test]
fn a_first_capture_reads_the_window_and_an_unchanged_one_reads_nothing() {
    let models = FakeModels::reading("fn main() {");
    let mut analyzer = analyzer(&models);
    let cancel = AtomicBool::new(false);

    let first = analyzer.analyse(line_at(10, vec![]), on_battery(&cancel));
    assert_eq!(first.text.unwrap().lines, ["fn main() {"]);
    assert_eq!(first.damage_permille, 1000);

    let unchanged = analyzer.analyse(
        line_at(10, vec![Rect::new(0, 0, 200, 100)]),
        on_battery(&cancel),
    );
    assert!(unchanged.text.is_none());
    assert_eq!(models.ocr_calls(), 1);
}

#[test]
fn a_scrolled_line_comes_from_the_line_cache() {
    let models = FakeModels::reading("fn main() {");
    let mut analyzer = analyzer(&models);
    let cancel = AtomicBool::new(false);
    analyzer.analyse(line_at(10, vec![]), on_battery(&cancel));

    let scrolled = analyzer.analyse(
        line_at(60, vec![Rect::new(0, 0, 200, 100)]),
        on_battery(&cancel),
    );
    assert_eq!(scrolled.text.unwrap().lines, ["fn main() {"]);
    assert_eq!(models.ocr_calls(), 1);
}

#[test]
fn a_secret_drops_all_text_and_empties_the_line_cache() {
    let models = FakeModels::reading(AWS_KEY_LINE);
    let mut analyzer = analyzer(&models);
    let cancel = AtomicBool::new(false);

    let outcome = analyzer.analyse(line_at(10, vec![]), plugged_in(&cancel));
    assert!(outcome.secret_found);
    assert!(outcome.text.is_none());
    assert!(outcome.description.is_none());
    let observations = outcome.observations();
    assert_eq!(observations.len(), 1);
    assert!(
        matches!(&observations[0].kind, ObservationKind::Frame(record) if record.line_count == 0)
    );

    analyzer.analyse(
        line_at(60, vec![Rect::new(0, 0, 200, 100)]),
        plugged_in(&cancel),
    );
    assert_eq!(models.ocr_calls(), 2);
}

#[test]
fn a_secret_in_the_caption_is_dropped_too() {
    let models = FakeModels::reading("ok");
    *models.caption.lock().unwrap() = AWS_KEY_LINE.to_owned();
    let mut analyzer = analyzer(&models);
    let cancel = AtomicBool::new(false);
    let outcome = analyzer.analyse(line_at(10, vec![]), plugged_in(&cancel));
    assert!(outcome.secret_found);
    assert!(outcome.description.is_none());
}

#[test]
fn a_low_text_frame_is_described_unless_on_battery() {
    let models = FakeModels::reading("cat.png");
    let cancel = AtomicBool::new(false);

    let battery = analyzer(&models).analyse(line_at(10, vec![]), on_battery(&cancel));
    assert!(battery.description.is_none());
    assert_eq!(models.describe_calls(), 0);

    let outcome = analyzer(&models).analyse(line_at(10, vec![]), plugged_in(&cancel));
    assert_eq!(models.describe_calls(), 1);
    let observations = outcome.observations();
    let sources: Vec<TextSource> = observations
        .iter()
        .filter_map(|observation| match &observation.kind {
            ObservationKind::Text { source, .. } => Some(*source),
            _ => None,
        })
        .collect();
    assert_eq!(sources, [TextSource::Ocr, TextSource::Vision]);
    let ObservationKind::Frame(record) = &observations[2].kind else {
        panic!("the last observation is the frame record");
    };
    assert_eq!(record.trigger, Trigger::FocusChange);
    assert_eq!(
        (record.width, record.height, record.line_count),
        (200, 100, 1)
    );
    assert_eq!(record.damage_permille, 1000);
}

#[test]
fn a_long_text_frame_is_not_described() {
    let models = FakeModels::reading(&"word ".repeat(60));
    let cancel = AtomicBool::new(false);
    analyzer(&models).analyse(line_at(10, vec![]), plugged_in(&cancel));
    assert_eq!(models.describe_calls(), 0);
}

#[test]
fn a_cancelled_capture_calls_no_model() {
    let models = FakeModels::reading("text");
    let cancel = AtomicBool::new(true);
    let outcome = analyzer(&models).analyse(line_at(10, vec![]), plugged_in(&cancel));
    assert!(outcome.cancelled);
    assert_eq!(models.ocr_calls() + models.describe_calls(), 0);
}

#[test]
fn text_that_ocr_missed_is_read_on_the_next_capture() {
    let models = FakeModels::reading("fn main() {");
    models.ocr_fails.store(true, Ordering::Relaxed);
    let mut analyzer = analyzer(&models);
    let cancel = AtomicBool::new(false);
    let failed = analyzer.analyse(line_at(10, vec![]), on_battery(&cancel));
    assert_eq!(failed.text.unwrap().lines, Vec::<String>::new());

    models.ocr_fails.store(false, Ordering::Relaxed);
    let unchanged = analyzer.analyse(line_at(10, vec![]), on_battery(&cancel));
    assert_eq!(unchanged.text.unwrap().lines, ["fn main() {"]);
}

#[test]
fn text_skipped_by_a_cancel_is_read_on_the_next_capture() {
    let models = FakeModels::reading("fn main() {");
    let mut analyzer = analyzer(&models);
    let cancel = AtomicBool::new(true);
    analyzer.analyse(line_at(10, vec![]), on_battery(&cancel));
    cancel.store(false, Ordering::Relaxed);
    let next = analyzer.analyse(line_at(10, vec![]), on_battery(&cancel));
    assert_eq!(next.text.unwrap().lines, ["fn main() {"]);
}

#[test]
fn the_frame_memfd_is_zero_after_analysis() {
    let models = FakeModels::reading("fn main() {");
    let frame = line_at(10, vec![]);
    let reader = memfd_reader(&frame);
    assert!(!is_all_zero(&reader));
    let cancel = AtomicBool::new(false);
    analyzer(&models).analyse(frame, plugged_in(&cancel));
    assert!(is_all_zero(&reader));
}
