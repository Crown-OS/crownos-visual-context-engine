#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use context_core::{
    ActivityLabel, DocKey, InputProfile, Micros, Observation, ObservationKind, Segment, SegmentId,
    SegmentState, SessionState, TextSource, TimeRange, Timestamp,
};
use context_store::{Store, TextLine, Write, Writer};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("context-store-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn segment(id: u64, start: i64, end: i64) -> Segment {
    Segment {
        id: SegmentId(id),
        episode: None,
        app_key: "code-oss".into(),
        doc_key: DocKey::File("/home/u/crownpositor/src/main.rs".into()),
        title: "main.rs — crownpositor".into(),
        url: None,
        file_path: Some("/home/u/crownpositor/src/main.rs".into()),
        cwd: None,
        project: None,
        range: TimeRange::new(Timestamp::from_secs(start), Timestamp::from_secs(end)),
        active: Micros::from_secs(end - start),
        input: InputProfile::default(),
        label: ActivityLabel::Coding,
        label_confidence: 0.9,
        importance: 0.5,
        text_coverage: 0.0,
        summary: None,
        state: SegmentState::Closed,
    }
}

fn seed(writer: &Writer) {
    let handle = writer.handle();
    handle
        .send(Write::App {
            app_key: "code-oss".into(),
            app_id: "code-oss".into(),
            name: "Code - OSS".into(),
        })
        .unwrap();
    handle
        .send(Write::Segment(Box::new(segment(1, 100, 700))))
        .unwrap();
    handle
        .send(Write::Segment(Box::new(segment(2, 800, 1_400))))
        .unwrap();
    handle
        .send(Write::TextLines {
            segment: SegmentId(1),
            lines: vec![
                TextLine {
                    source: TextSource::Accessibility,
                    text: "fn damage_tracker(output: &Output)".into(),
                    t: Timestamp::from_secs(120),
                },
                TextLine {
                    source: TextSource::Ocr,
                    text: "let elements = render_elements(&space);".into(),
                    t: Timestamp::from_secs(130),
                },
            ],
        })
        .unwrap();
    handle
        .send(Write::Event {
            observation: Observation::new(
                Timestamp::from_secs(100),
                ObservationKind::Session(SessionState::Active),
            ),
            app_key: None,
            t_end: Timestamp::from_secs(100),
        })
        .unwrap();
    handle.flush().unwrap();
}

#[test]
fn rows_come_back_and_text_is_searchable() {
    let dir = scratch("roundtrip");
    let writer = Writer::spawn(Store::open_dir(&dir).unwrap()).unwrap();
    seed(&writer);
    let reader = Store::open_dir(&dir).unwrap();
    let segments = reader
        .segments_between(
            TimeRange::new(Timestamp(0), Timestamp::from_secs(10_000)),
            None,
            0.0,
        )
        .unwrap();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].segment, segment(1, 100, 700));
    let hits = reader.search_text("damage tracker", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].segment_id, SegmentId(1));
    assert!(hits[0].snippet.contains("«damage»"));
    assert_eq!(reader.search_locators("crownpositor", 10).unwrap().len(), 2);
    assert!(
        reader
            .search_text("\"); DROP TABLE segments; --", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(reader.count("events").unwrap(), 1);
    drop(writer);
}

#[test]
fn forgetting_a_range_cascades() {
    let dir = scratch("forget");
    let writer = Writer::spawn(Store::open_dir(&dir).unwrap()).unwrap();
    seed(&writer);
    let outcome = writer
        .handle()
        .forget(
            TimeRange::new(Timestamp::from_secs(0), Timestamp::from_secs(750)),
            None,
        )
        .unwrap();
    assert_eq!(outcome.segments, 1);
    assert_eq!(outcome.text_lines, 2);
    assert_eq!(outcome.events, 1);
    let reader = Store::open_dir(&dir).unwrap();
    assert_eq!(reader.count("segments").unwrap(), 1);
    assert_eq!(reader.count("text_lines").unwrap(), 0);
    assert!(reader.search_text("damage", 10).unwrap().is_empty());
    reader.integrity_check().unwrap();
}

#[test]
fn an_uncommitted_transaction_leaves_nothing_behind() {
    let dir = scratch("crash");
    {
        let writer = Writer::spawn(Store::open_dir(&dir).unwrap()).unwrap();
        seed(&writer);
    }
    {
        let mut store = Store::open_dir(&dir).unwrap();
        let connection = store.connection();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        connection
            .execute(
                "INSERT INTO events(kind, t_start, t_end, payload) VALUES ('crash', 1, 1, '{}')",
                [],
            )
            .unwrap();
        let _ = &mut store;
    }
    let reader = Store::open_dir(&dir).unwrap();
    reader.integrity_check().unwrap();
    assert_eq!(reader.count("events").unwrap(), 1);
    assert_eq!(reader.count("segments").unwrap(), 2);
}

#[test]
fn the_database_needs_its_key() {
    let dir = scratch("key");
    {
        let writer = Writer::spawn(Store::open_dir(&dir).unwrap()).unwrap();
        seed(&writer);
    }
    let wrong = Store::open(&dir.join("context.db"), &"00".repeat(32));
    if Store::is_encrypted() {
        assert!(wrong.is_err());
    } else {
        assert!(wrong.is_ok());
    }
    assert!(Store::open_dir(&dir).is_ok());
    let bytes = std::fs::read(dir.join("context.db")).unwrap();
    let has_plaintext = bytes
        .windows(9)
        .any(|window| window == &b"crownpositor"[..9]);
    assert_eq!(has_plaintext, !Store::is_encrypted());
}
