#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! What must never reach the store through the pipeline: a run of a frame's pixels, a secret
//! on screen, and a clipboard copy the source marked secret.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use context_capture::{
    AnalysisConditions, CaptureTarget, CapturedFrame, FrameAnalyzer, FrameMemory, ModelCall,
    ModelError, NoStructuredText, RecognizedLine, RegionCopy, ScreenModels, ScreenSummary,
};
use context_core::{
    FrameRecord, Micros, Observation, ObservationKind, Placement, SelectionKind, SessionState,
    TextSource, Timestamp, Trigger, WindowIdent, WindowRecord,
};
use context_eval::pixel_scan::{PixelNeedles, noise_pixels};
use context_policy::{Policy, VisualContext};
use context_store::{Store, Writer};
use proptest::prelude::*;
use rusqlite::types::ValueRef;

use crate::pipeline;
use crate::shared::Shared;

struct EchoModels {
    lines: Vec<String>,
}

impl ScreenModels for EchoModels {
    fn recognize_text(
        &self,
        region: &RegionCopy,
        _: &ModelCall<'_>,
    ) -> Result<Vec<RecognizedLine>, ModelError> {
        Ok(self
            .lines
            .iter()
            .map(|text| RecognizedLine {
                text: text.clone(),
                confidence: 0.9,
                bounds: region.bounds(),
            })
            .collect())
    }

    fn describe_screen(
        &self,
        _: &RegionCopy,
        _: &str,
        _: &str,
        _: &ModelCall<'_>,
    ) -> Result<ScreenSummary, ModelError> {
        Ok(ScreenSummary {
            content_kind: "image".to_owned(),
            caption: "Noise on a test window".to_owned(),
            visible_entities: vec!["VCGE".to_owned()],
        })
    }
}

fn scratch_dir(case: u64) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "contextd-frame-privacy-{}-{case}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn window(ident: &WindowIdent) -> WindowRecord {
    WindowRecord {
        ident: ident.clone(),
        app_id: "org.crownos.Test".to_owned(),
        title: "Noise".to_owned(),
        placement: Some(Placement {
            x: 0,
            y: 0,
            width: 200,
            height: 150,
        }),
        ..WindowRecord::default()
    }
}

fn captured_frame(ident: &WindowIdent, pixels: &[u8], width: u32, height: u32) -> CapturedFrame {
    let mut memory = FrameMemory::allocate(pixels.len()).unwrap();
    assert!(memory.write_at(0, pixels));
    memory.seal().unwrap();
    let target = CaptureTarget {
        window: ident.clone(),
        trigger: Trigger::FocusChange,
        app_key: "org.crownos.test".to_owned(),
        app_name: "Test".to_owned(),
        title: "Noise".to_owned(),
    };
    CapturedFrame::new(target, width, height, width as usize * 4, vec![], memory).unwrap()
}

/// Every TEXT and BLOB value of every table, read through the key.
fn database_values(state_dir: &Path) -> Vec<Vec<u8>> {
    let store = Store::open_dir(state_dir).unwrap();
    let connection = store.connection();
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut values = Vec::new();
    for table in tables {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for column in 0..columns {
                match row.get_ref(column).unwrap() {
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => values.push(bytes.to_vec()),
                    _ => {}
                }
            }
        }
    }
    values
}

fn files_in(dir: &Path) -> Vec<Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| std::fs::read(entry.path()).unwrap())
        .collect()
}

fn run_case(case: u64, width: u32, height: u32, lines: Vec<String>) {
    let state_dir = scratch_dir(case);
    let pixels = noise_pixels(case, width, height);
    let mut needles = PixelNeedles::default();
    needles.add_xrgb(&pixels);

    let writer = Writer::spawn(Store::open_dir(&state_dir).unwrap()).unwrap();
    let section = VisualContext {
        enabled: true,
        ..VisualContext::default()
    };
    let (outlet, observations) = mpsc::sync_channel(64);
    let (events, _outbox) = tokio::sync::mpsc::unbounded_channel();
    let pipeline = pipeline::spawn(
        observations,
        Policy::from_section(&section).unwrap(),
        writer.handle(),
        Arc::new(Mutex::new(Shared::new(true))),
        events,
        None,
    )
    .unwrap();

    let ident = WindowIdent(format!("window-{case}"));
    let now = Timestamp::now();
    outlet
        .send(Observation::new(
            now,
            ObservationKind::WindowOpened(window(&ident)),
        ))
        .unwrap();
    outlet
        .send(Observation::new(
            now,
            ObservationKind::Focus(Some(ident.clone())),
        ))
        .unwrap();

    let models = Arc::new(EchoModels { lines });
    let mut analyzer = FrameAnalyzer::new(models, Box::new(NoStructuredText));
    let cancel = AtomicBool::new(false);
    let outcome = analyzer.analyse(
        captured_frame(&ident, &pixels, width, height),
        AnalysisConditions {
            on_battery: false,
            cancel: &cancel,
        },
    );
    for observation in outcome.observations() {
        outlet.send(observation).unwrap();
    }
    let segment_opens = now.plus(Micros::from_secs(3));
    outlet
        .send(Observation::new(
            segment_opens,
            ObservationKind::Session(SessionState::Active),
        ))
        .unwrap();
    drop(outlet);
    pipeline.drain();
    drop(writer);

    let values = database_values(&state_dir);
    let frame_rows = Store::open_dir(&state_dir)
        .unwrap()
        .count("frames")
        .unwrap();
    assert_eq!(frame_rows, 1, "the capture reached the store");
    assert!(!values.is_empty());
    for value in values.iter().chain(&files_in(&state_dir)) {
        assert!(
            !needles.found_in(value),
            "a run of frame pixels is in the store"
        );
    }
    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn the_scan_finds_a_planted_run_of_pixels() {
    let pixels = noise_pixels(1, 32, 32);
    let mut needles = PixelNeedles::default();
    needles.add_xrgb(&pixels);
    let mut value = b"caption: ".to_vec();
    value.extend_from_slice(&pixels[200..240]);
    assert!(needles.found_in(&value));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]

    #[test]
    fn no_code_path_writes_frame_bytes_to_the_store(
        case in any::<u64>(),
        width in 32_u32..=200,
        height in 32_u32..=150,
        lines in proptest::collection::vec("[a-z ]{1,40}", 0..4),
    ) {
        run_case(case, width, height, lines);
    }
}

/// Sends `kinds` through a real pipeline into a fresh store, one second apart, and waits until
/// everything is written. A segment opens two seconds after the focus.
fn run_pipeline(case: u64, kinds: Vec<ObservationKind>) -> PathBuf {
    let state_dir = scratch_dir(case);
    let writer = Writer::spawn(Store::open_dir(&state_dir).unwrap()).unwrap();
    let section = VisualContext {
        enabled: true,
        ..VisualContext::default()
    };
    let (outlet, observations) = mpsc::sync_channel(64);
    let (events, _outbox) = tokio::sync::mpsc::unbounded_channel();
    let pipeline = pipeline::spawn(
        observations,
        Policy::from_section(&section).unwrap(),
        writer.handle(),
        Arc::new(Mutex::new(Shared::new(true))),
        events,
        None,
    )
    .unwrap();
    let now = Timestamp::now();
    for (offset, kind) in kinds.into_iter().enumerate() {
        let t = now.plus(Micros::from_secs(i64::try_from(offset).unwrap()));
        outlet.send(Observation::new(t, kind)).unwrap();
    }
    drop(outlet);
    pipeline.drain();
    drop(writer);
    state_dir
}

fn ocr_text(window: &WindowIdent, line: &str) -> ObservationKind {
    ObservationKind::Text {
        window: window.clone(),
        source: TextSource::Ocr,
        url: None,
        lines: vec![line.to_owned()],
    }
}

fn count(state_dir: &Path, sql: &str) -> i64 {
    Store::open_dir(state_dir)
        .unwrap()
        .connection()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_secret_copy_and_secret_text_are_not_stored() {
    let ident = WindowIdent("terminal".to_owned());
    let state_dir = run_pipeline(
        u64::MAX,
        vec![
            ObservationKind::WindowOpened(window(&ident)),
            ObservationKind::Focus(Some(ident.clone())),
            ObservationKind::Selection {
                kind: SelectionKind::Clipboard,
                window: Some(ident.clone()),
                mime_types: vec!["text/plain".to_owned()],
                size_hint: 40,
                secret: true,
            },
            ocr_text(&ident, "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"),
            ObservationKind::Session(SessionState::Active),
        ],
    );
    assert_eq!(
        count(
            &state_dir,
            "SELECT count(*) FROM events WHERE kind = 'selection'"
        ),
        0
    );
    assert_eq!(
        count(&state_dir, "SELECT count(*) FROM segments"),
        1,
        "the window was observed"
    );
    assert_eq!(count(&state_dir, "SELECT count(*) FROM text_lines"), 0);
    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn output_is_dropped_when_a_gate_closes_between_capture_and_store() {
    let ident = WindowIdent("browser".to_owned());
    let mut flagged = window(&ident);
    flagged.sensitivity.password_input = true;
    let state_dir = run_pipeline(
        u64::MAX - 1,
        vec![
            ObservationKind::WindowOpened(window(&ident)),
            ObservationKind::Focus(Some(ident.clone())),
            ocr_text(&ident, "Sign in to your account"),
            ObservationKind::WindowChanged(flagged),
            ObservationKind::Session(SessionState::Active),
        ],
    );
    assert_eq!(count(&state_dir, "SELECT count(*) FROM segments"), 1);
    assert_eq!(count(&state_dir, "SELECT count(*) FROM text_lines"), 0);
    let _ = std::fs::remove_dir_all(&state_dir);
}

#[test]
fn a_capture_that_saw_a_secret_drops_the_waiting_output_and_holds_the_window() {
    let ident = WindowIdent("editor".to_owned());
    let frame = |secret_seen| {
        ObservationKind::Frame(FrameRecord {
            window: ident.clone(),
            trigger: Trigger::TypingPause,
            width: 200,
            height: 150,
            damage_permille: 100,
            line_count: 0,
            secret_seen,
        })
    };
    let state_dir = run_pipeline(
        u64::MAX - 2,
        vec![
            ObservationKind::WindowOpened(window(&ident)),
            ObservationKind::Focus(Some(ident.clone())),
            ocr_text(&ident, "notes before the key appeared"),
            frame(true),
            ObservationKind::Session(SessionState::Active),
            ocr_text(&ident, "text read after the secret"),
            frame(false),
        ],
    );
    assert_eq!(count(&state_dir, "SELECT count(*) FROM segments"), 1);
    assert_eq!(count(&state_dir, "SELECT count(*) FROM text_lines"), 0);
    assert_eq!(count(&state_dir, "SELECT count(*) FROM frames"), 0);
    let _ = std::fs::remove_dir_all(&state_dir);
}
