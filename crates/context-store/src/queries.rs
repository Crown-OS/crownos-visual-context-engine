//! Reads and writes, as plain functions over a connection so the writer thread and the readers
//! share them.

use context_core::{
    ActivityLabel, DocKey, Episode, EpisodeId, EpisodeLevel, InputProfile, Observation, Segment,
    SegmentId, SegmentState, TextSource, TimeRange, Timestamp,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::store::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub kind: String,
    pub app_key: Option<String>,
    pub window_ident: Option<String>,
    pub range: TimeRange,
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRow {
    pub segment: Segment,
    pub app_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    pub source: TextSource,
    pub text: String,
    pub t: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextHit {
    pub segment_id: SegmentId,
    pub snippet: String,
    pub rank: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ForgetOutcome {
    pub events: u64,
    pub segments: u64,
    pub text_lines: u64,
    pub frames: u64,
    pub episodes_removed: u64,
}

pub(crate) fn upsert_app(
    connection: &Connection,
    app_key: &str,
    app_id: &str,
    name: &str,
) -> Result<i64, StoreError> {
    connection.execute(
        "INSERT INTO apps(app_key, app_id, name) VALUES (?1, ?2, ?3)
         ON CONFLICT(app_key) DO UPDATE SET app_id = excluded.app_id, name = excluded.name",
        params![app_key, app_id, name],
    )?;
    Ok(
        connection.query_row("SELECT id FROM apps WHERE app_key = ?1", [app_key], |row| {
            row.get(0)
        })?,
    )
}

fn app_id_of(connection: &Connection, app_key: &str) -> Result<Option<i64>, StoreError> {
    Ok(connection
        .query_row("SELECT id FROM apps WHERE app_key = ?1", [app_key], |row| {
            row.get(0)
        })
        .optional()?)
}

/// One row of the event log. `kind` is the observation's variant name; the payload is JSON.
pub(crate) fn insert_event(
    connection: &Connection,
    observation: &Observation,
    app_key: Option<&str>,
    t_end: Timestamp,
) -> Result<i64, StoreError> {
    let kind = kind_name(observation);
    let app_id = match app_key {
        Some(key) => app_id_of(connection, key)?,
        None => None,
    };
    let window = observation.window().map(|ident| ident.0.clone());
    let payload = serde_json::to_string(&observation.kind)?;
    connection.execute(
        "INSERT INTO events(kind, app_id, window_ident, t_start, t_end, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![kind, app_id, window, observation.t.0, t_end.0, payload],
    )?;
    Ok(connection.last_insert_rowid())
}

const fn kind_name(observation: &Observation) -> &'static str {
    use context_core::ObservationKind::*;
    match &observation.kind {
        WindowOpened(_) => "window_opened",
        WindowChanged(_) => "window_changed",
        WindowClosed(_) => "window_closed",
        Focus(_) => "focus",
        Session(_) => "session",
        InputActivity { .. } => "input_bucket",
        Selection { .. } => "selection",
        Paused { .. } => "paused",
        Text { .. } => "text",
        Notification { .. } => "notification",
    }
}

pub(crate) fn events_between(
    connection: &Connection,
    range: TimeRange,
    limit: u32,
) -> Result<Vec<EventRow>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT e.id, e.kind, a.app_key, e.window_ident, e.t_start, e.t_end, e.payload
         FROM events e LEFT JOIN apps a ON a.id = e.app_id
         WHERE e.t_start < ?2 AND e.t_end >= ?1
         ORDER BY e.t_start LIMIT ?3",
    )?;
    let rows = statement.query_map(params![range.start.0, range.end.0, limit], |row| {
        Ok(EventRow {
            id: row.get(0)?,
            kind: row.get(1)?,
            app_key: row.get(2)?,
            window_ident: row.get(3)?,
            range: TimeRange::new(Timestamp(row.get(4)?), Timestamp(row.get(5)?)),
            payload: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn upsert_segment(connection: &Connection, segment: &Segment) -> Result<(), StoreError> {
    let app_id = app_id_of(connection, &segment.app_key)?
        .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
    let input = serde_json::to_string(&segment.input)?;
    connection.execute(
        "INSERT INTO segments(id, episode_id, app_id, doc_key, title, url, file_path, cwd, project_id,
                              t_start, t_end, active_us, input_profile, activity_label, label_conf,
                              importance, text_coverage, summary, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(id) DO UPDATE SET
            episode_id = excluded.episode_id, title = excluded.title, url = excluded.url,
            file_path = excluded.file_path, cwd = excluded.cwd, project_id = excluded.project_id,
            t_end = excluded.t_end, active_us = excluded.active_us, input_profile = excluded.input_profile,
            activity_label = excluded.activity_label, label_conf = excluded.label_conf,
            importance = excluded.importance, text_coverage = excluded.text_coverage,
            summary = excluded.summary, state = excluded.state",
        params![
            segment.id.0 as i64,
            segment.episode.map(|id| id.0 as i64),
            app_id,
            segment.doc_key.to_string(),
            segment.title,
            segment.url,
            segment.file_path,
            segment.cwd,
            segment.project.map(|id| id.0 as i64),
            segment.range.start.0,
            segment.range.end.0,
            segment.active.0,
            input,
            segment.label.as_str(),
            f64::from(segment.label_confidence),
            f64::from(segment.importance),
            f64::from(segment.text_coverage),
            segment.summary,
            state_name(segment.state),
        ],
    )?;
    Ok(())
}

const fn state_name(state: SegmentState) -> &'static str {
    match state {
        SegmentState::Open => "open",
        SegmentState::Closed => "closed",
        SegmentState::Summarized => "summarized",
    }
}

fn parse_state(text: &str) -> SegmentState {
    match text {
        "closed" => SegmentState::Closed,
        "summarized" => SegmentState::Summarized,
        _ => SegmentState::Open,
    }
}

fn parse_doc_key(text: &str) -> DocKey {
    match text.split_once(':') {
        Some(("doc", rest)) => DocKey::Document(rest.to_owned()),
        Some(("url", rest)) => DocKey::Url(rest.to_owned()),
        Some(("file", rest)) => DocKey::File(rest.to_owned()),
        Some(("dir", rest)) => DocKey::Directory(rest.to_owned()),
        Some(("title", rest)) => DocKey::Title(rest.to_owned()),
        _ => DocKey::Title(text.to_owned()),
    }
}

fn segment_from_row(row: &Row<'_>) -> rusqlite::Result<SegmentRow> {
    let input: String = row.get(12)?;
    let label: String = row.get(13)?;
    let doc_key: String = row.get(3)?;
    let state: String = row.get(18)?;
    let app_key: String = row.get(19)?;
    Ok(SegmentRow {
        segment: Segment {
            id: SegmentId(row.get::<_, i64>(0)? as u64),
            episode: row.get::<_, Option<i64>>(1)?.map(|id| EpisodeId(id as u64)),
            app_key: app_key.clone(),
            doc_key: parse_doc_key(&doc_key),
            title: row.get(4)?,
            url: row.get(5)?,
            file_path: row.get(6)?,
            cwd: row.get(7)?,
            project: row
                .get::<_, Option<i64>>(8)?
                .map(|id| context_core::ProjectId(id as u64)),
            range: TimeRange::new(Timestamp(row.get(9)?), Timestamp(row.get(10)?)),
            active: context_core::Micros(row.get(11)?),
            input: serde_json::from_str::<InputProfile>(&input).unwrap_or_default(),
            label: ActivityLabel::parse(&label).unwrap_or_default(),
            label_confidence: row.get::<_, f32>(14)?,
            importance: row.get::<_, f32>(15)?,
            text_coverage: row.get::<_, f32>(16)?,
            summary: row.get(17)?,
            state: parse_state(&state),
        },
        app_key,
    })
}

const SEGMENT_COLUMNS: &str =
    "s.id, s.episode_id, s.app_id, s.doc_key, s.title, s.url, s.file_path, s.cwd, s.project_id,
    s.t_start, s.t_end, s.active_us, s.input_profile, s.activity_label, s.label_conf, s.importance,
    s.text_coverage, s.summary, s.state, a.app_key";

pub(crate) fn segments_between(
    connection: &Connection,
    range: TimeRange,
    app_key: Option<&str>,
    min_importance: f32,
) -> Result<Vec<SegmentRow>, StoreError> {
    let sql = format!(
        "SELECT {SEGMENT_COLUMNS} FROM segments s JOIN apps a ON a.id = s.app_id
         WHERE s.t_start < ?2 AND s.t_end > ?1 AND s.importance >= ?3
           AND (?4 IS NULL OR a.app_key = ?4)
         ORDER BY s.t_start"
    );
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement.query_map(
        params![
            range.start.0,
            range.end.0,
            f64::from(min_importance),
            app_key
        ],
        segment_from_row,
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn segment(
    connection: &Connection,
    id: SegmentId,
) -> Result<Option<SegmentRow>, StoreError> {
    let sql = format!(
        "SELECT {SEGMENT_COLUMNS} FROM segments s JOIN apps a ON a.id = s.app_id WHERE s.id = ?1"
    );
    Ok(connection
        .query_row(&sql, [id.0 as i64], segment_from_row)
        .optional()?)
}

pub(crate) fn upsert_episode(connection: &Connection, episode: &Episode) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO episodes(id, level, parent_id, t_start, t_end, title, summary, activity_label,
                              project_id, importance, confidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET
            t_end = excluded.t_end, title = excluded.title, summary = excluded.summary,
            activity_label = excluded.activity_label, project_id = excluded.project_id,
            importance = excluded.importance, confidence = excluded.confidence",
        params![
            episode.id.0 as i64,
            level_name(episode.level),
            episode.parent.map(|id| id.0 as i64),
            episode.range.start.0,
            episode.range.end.0,
            episode.title,
            episode.summary,
            episode.label.as_str(),
            episode.project.map(|id| id.0 as i64),
            f64::from(episode.importance),
            f64::from(episode.confidence),
        ],
    )?;
    for segment in &episode.segments {
        connection.execute(
            "UPDATE segments SET episode_id = ?1 WHERE id = ?2",
            params![episode.id.0 as i64, segment.0 as i64],
        )?;
    }
    Ok(())
}

const fn level_name(level: EpisodeLevel) -> &'static str {
    match level {
        EpisodeLevel::Task => "task",
        EpisodeLevel::Day => "day",
        EpisodeLevel::Week => "week",
    }
}

fn parse_level(text: &str) -> EpisodeLevel {
    match text {
        "day" => EpisodeLevel::Day,
        "week" => EpisodeLevel::Week,
        _ => EpisodeLevel::Task,
    }
}

pub(crate) fn episodes_between(
    connection: &Connection,
    range: TimeRange,
) -> Result<Vec<Episode>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT id, level, parent_id, t_start, t_end, title, summary, activity_label, project_id,
                importance, confidence
         FROM episodes WHERE t_start < ?2 AND t_end > ?1 ORDER BY t_start",
    )?;
    let mut episodes: Vec<Episode> = statement
        .query_map(params![range.start.0, range.end.0], |row| {
            let level: String = row.get(1)?;
            let label: String = row.get(7)?;
            Ok(Episode {
                id: EpisodeId(row.get::<_, i64>(0)? as u64),
                level: parse_level(&level),
                parent: row.get::<_, Option<i64>>(2)?.map(|id| EpisodeId(id as u64)),
                range: TimeRange::new(Timestamp(row.get(3)?), Timestamp(row.get(4)?)),
                title: row.get(5)?,
                summary: row.get(6)?,
                label: ActivityLabel::parse(&label).unwrap_or_default(),
                project: row
                    .get::<_, Option<i64>>(8)?
                    .map(|id| context_core::ProjectId(id as u64)),
                importance: row.get::<_, f32>(9)?,
                confidence: row.get::<_, f32>(10)?,
                segments: Vec::new(),
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut members = connection
        .prepare_cached("SELECT id FROM segments WHERE episode_id = ?1 ORDER BY t_start")?;
    for episode in &mut episodes {
        episode.segments = members
            .query_map([episode.id.0 as i64], |row| {
                Ok(SegmentId(row.get::<_, i64>(0)? as u64))
            })?
            .collect::<Result<_, _>>()?;
    }
    Ok(episodes)
}

pub(crate) fn insert_text_lines(
    connection: &Connection,
    segment: SegmentId,
    lines: &[TextLine],
) -> Result<u64, StoreError> {
    let mut inserted = 0;
    let mut statement = connection.prepare_cached(
        "INSERT INTO text_lines(segment_id, source, line_hash, text, t_first, t_last)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(segment_id, line_hash) DO UPDATE SET t_last = excluded.t_last",
    )?;
    for line in lines {
        let hash = line_hash(&line.text);
        inserted += statement.execute(params![
            segment.0 as i64,
            source_name(line.source),
            hash,
            line.text,
            line.t.0
        ])? as u64;
    }
    Ok(inserted)
}

const fn source_name(source: TextSource) -> &'static str {
    match source {
        TextSource::CrownUi => "crownui",
        TextSource::Accessibility => "a11y",
        TextSource::Ocr => "ocr",
        TextSource::Vision => "vlm",
    }
}

/// FNV-1a over the trimmed line, enough to dedup within a segment.
fn line_hash(text: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.trim().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

pub(crate) fn search_text(
    connection: &Connection,
    query: &str,
    limit: u32,
) -> Result<Vec<TextHit>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT l.segment_id, snippet(text_fts, 0, '«', '»', '…', 12), bm25(text_fts)
         FROM text_fts JOIN text_lines l ON l.id = text_fts.rowid
         WHERE text_fts MATCH ?1 ORDER BY bm25(text_fts) LIMIT ?2",
    )?;
    let rows = statement.query_map(params![fts_query(query), limit], |row| {
        Ok(TextHit {
            segment_id: SegmentId(row.get::<_, i64>(0)? as u64),
            snippet: row.get(1)?,
            rank: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn search_locators(
    connection: &Connection,
    query: &str,
    limit: u32,
) -> Result<Vec<SegmentId>, StoreError> {
    if query.chars().count() < 3 {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare_cached(
        "SELECT rowid FROM locator_fts WHERE locator_fts MATCH ?1 ORDER BY bm25(locator_fts) LIMIT ?2",
    )?;
    let rows = statement.query_map(params![fts_query(query), limit], |row| {
        Ok(SegmentId(row.get::<_, i64>(0)? as u64))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every word quoted, so user text never reaches FTS5's query syntax.
fn fts_query(text: &str) -> String {
    text.split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Removes evidence in `range` and whatever rested only on it.
pub(crate) fn forget(
    connection: &Connection,
    range: TimeRange,
    app_key: Option<&str>,
) -> Result<ForgetOutcome, StoreError> {
    let app_id = match app_key {
        Some(key) => app_id_of(connection, key)?,
        None => None,
    };
    let segment_ids: Vec<i64> = {
        let mut statement = connection.prepare_cached(
            "SELECT id FROM segments WHERE t_start < ?2 AND t_end > ?1 AND (?3 IS NULL OR app_id = ?3)",
        )?;
        statement
            .query_map(params![range.start.0, range.end.0, app_id], |row| {
                row.get(0)
            })?
            .collect::<Result<_, _>>()?
    };
    let mut outcome = ForgetOutcome::default();
    for id in &segment_ids {
        outcome.text_lines +=
            connection.execute("DELETE FROM text_lines WHERE segment_id = ?1", [id])? as u64;
        outcome.frames +=
            connection.execute("DELETE FROM frames WHERE segment_id = ?1", [id])? as u64;
        connection.execute("DELETE FROM mentions WHERE segment_id = ?1", [id])?;
        connection.execute(
            "DELETE FROM evidence WHERE source_kind = 'segment' AND source_id = ?1",
            [id.to_string()],
        )?;
    }
    for id in &segment_ids {
        outcome.segments += connection.execute("DELETE FROM segments WHERE id = ?1", [id])? as u64;
    }
    outcome.events += connection.execute(
        "DELETE FROM events WHERE t_start < ?2 AND t_end > ?1 AND (?3 IS NULL OR app_id = ?3)",
        params![range.start.0, range.end.0, app_id],
    )? as u64;
    outcome.episodes_removed += connection.execute(
        "DELETE FROM episodes WHERE id NOT IN (SELECT DISTINCT episode_id FROM segments WHERE episode_id IS NOT NULL)
           AND level = 'task'",
        [],
    )? as u64;
    Ok(outcome)
}

pub(crate) fn save_working_memory(
    connection: &Connection,
    snapshot: &[u8],
    t: Timestamp,
) -> Result<(), StoreError> {
    connection.execute(
        "INSERT INTO working_memory(id, snapshot, t) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET snapshot = excluded.snapshot, t = excluded.t",
        params![snapshot, t.0],
    )?;
    Ok(())
}

pub(crate) fn load_working_memory(
    connection: &Connection,
) -> Result<Option<(Vec<u8>, Timestamp)>, StoreError> {
    Ok(connection
        .query_row(
            "SELECT snapshot, t FROM working_memory WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, Timestamp(row.get(1)?))),
        )
        .optional()?)
}

/// Appends to the hash chain: each row's hash covers the previous one.
pub(crate) fn audit(
    connection: &Connection,
    actor: &str,
    action: &str,
    detail: &str,
    t: Timestamp,
) -> Result<(), StoreError> {
    let previous: String = connection
        .query_row(
            "SELECT hash FROM audit ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let hash = format!(
        "{:016x}",
        line_hash(&format!(
            "{previous}|{t}|{actor}|{action}|{detail}",
            t = t.0
        ))
    );
    connection.execute(
        "INSERT INTO audit(t, actor, action, detail, previous_hash, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![t.0, actor, action, detail, previous, hash],
    )?;
    Ok(())
}

pub(crate) fn count(connection: &Connection, table: &str) -> Result<u64, StoreError> {
    let allowed = [
        "events",
        "segments",
        "episodes",
        "text_lines",
        "frames",
        "audit",
    ];
    if !allowed.contains(&table) {
        return Ok(0);
    }
    Ok(
        connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })? as u64,
    )
}
