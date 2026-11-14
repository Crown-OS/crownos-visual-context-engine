//! The schema, applied in order; `user_version` says how far a database has come.

use rusqlite::Connection;

use crate::store::StoreError;

const MIGRATIONS: &[&str] = &[V1];

const V1: &str = r#"
CREATE TABLE apps(
    id INTEGER PRIMARY KEY,
    app_key TEXT NOT NULL UNIQUE,
    app_id TEXT NOT NULL,
    name TEXT NOT NULL,
    category TEXT,
    sandbox_engine TEXT
) STRICT;

CREATE TABLE events(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    app_id INTEGER REFERENCES apps(id),
    window_ident TEXT,
    t_start INTEGER NOT NULL,
    t_end INTEGER NOT NULL,
    payload TEXT NOT NULL,
    retention TEXT NOT NULL DEFAULT 'raw'
) STRICT;
CREATE INDEX events_t_start ON events(t_start);
CREATE INDEX events_kind_t_start ON events(kind, t_start);

CREATE TABLE episodes(
    id INTEGER PRIMARY KEY,
    level TEXT NOT NULL,
    parent_id INTEGER REFERENCES episodes(id),
    t_start INTEGER NOT NULL,
    t_end INTEGER NOT NULL,
    title TEXT NOT NULL,
    summary TEXT,
    activity_label TEXT NOT NULL,
    project_id INTEGER,
    importance REAL NOT NULL DEFAULT 0,
    confidence REAL NOT NULL DEFAULT 0,
    model TEXT,
    version INTEGER NOT NULL DEFAULT 1
) STRICT;
CREATE INDEX episodes_t_start ON episodes(t_start);
CREATE VIRTUAL TABLE episode_fts USING fts5(title, summary, content='episodes', content_rowid='id');

CREATE TABLE segments(
    id INTEGER PRIMARY KEY,
    episode_id INTEGER REFERENCES episodes(id),
    app_id INTEGER NOT NULL REFERENCES apps(id),
    doc_key TEXT NOT NULL,
    title TEXT NOT NULL,
    url TEXT,
    file_path TEXT,
    cwd TEXT,
    project_id INTEGER,
    t_start INTEGER NOT NULL,
    t_end INTEGER NOT NULL,
    active_us INTEGER NOT NULL,
    input_profile TEXT NOT NULL,
    activity_label TEXT NOT NULL,
    label_conf REAL NOT NULL,
    importance REAL NOT NULL,
    text_coverage REAL NOT NULL DEFAULT 0,
    summary TEXT,
    summary_model TEXT,
    state TEXT NOT NULL
) STRICT;
CREATE INDEX segments_t_start ON segments(t_start);
CREATE INDEX segments_app ON segments(app_id, t_start);
CREATE INDEX segments_episode ON segments(episode_id);
CREATE VIRTUAL TABLE locator_fts USING fts5(url, file_path, title, content='segments', content_rowid='id', tokenize='trigram');

CREATE TABLE frames(
    id INTEGER PRIMARY KEY,
    segment_id INTEGER NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    t INTEGER NOT NULL,
    trigger TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    damage_ratio REAL NOT NULL,
    thumb_path TEXT,
    line_count INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX frames_segment ON frames(segment_id);

CREATE TABLE text_lines(
    id INTEGER PRIMARY KEY,
    segment_id INTEGER NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    frame_id INTEGER REFERENCES frames(id) ON DELETE SET NULL,
    source TEXT NOT NULL,
    line_hash INTEGER NOT NULL,
    text TEXT NOT NULL,
    bbox TEXT,
    conf REAL NOT NULL DEFAULT 1,
    t_first INTEGER NOT NULL,
    t_last INTEGER NOT NULL,
    UNIQUE(segment_id, line_hash)
) STRICT;
CREATE VIRTUAL TABLE text_fts USING fts5(text, content='text_lines', content_rowid='id', tokenize='unicode61 remove_diacritics 2');

CREATE TRIGGER text_lines_ai AFTER INSERT ON text_lines BEGIN
    INSERT INTO text_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER text_lines_ad AFTER DELETE ON text_lines BEGIN
    INSERT INTO text_fts(text_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER segments_ai AFTER INSERT ON segments BEGIN
    INSERT INTO locator_fts(rowid, url, file_path, title) VALUES (new.id, new.url, new.file_path, new.title);
END;
CREATE TRIGGER segments_ad AFTER DELETE ON segments BEGIN
    INSERT INTO locator_fts(locator_fts, rowid, url, file_path, title) VALUES ('delete', old.id, old.url, old.file_path, old.title);
END;
CREATE TRIGGER segments_au AFTER UPDATE OF url, file_path, title ON segments BEGIN
    INSERT INTO locator_fts(locator_fts, rowid, url, file_path, title) VALUES ('delete', old.id, old.url, old.file_path, old.title);
    INSERT INTO locator_fts(rowid, url, file_path, title) VALUES (new.id, new.url, new.file_path, new.title);
END;
CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episode_fts(rowid, title, summary) VALUES (new.id, new.title, new.summary);
END;
CREATE TRIGGER episodes_ad AFTER DELETE ON episodes BEGIN
    INSERT INTO episode_fts(episode_fts, rowid, title, summary) VALUES ('delete', old.id, old.title, old.summary);
END;
CREATE TRIGGER episodes_au AFTER UPDATE OF title, summary ON episodes BEGIN
    INSERT INTO episode_fts(episode_fts, rowid, title, summary) VALUES ('delete', old.id, old.title, old.summary);
    INSERT INTO episode_fts(rowid, title, summary) VALUES (new.id, new.title, new.summary);
END;

CREATE TABLE entities(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    attributes TEXT NOT NULL DEFAULT '{}',
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    mention_count INTEGER NOT NULL DEFAULT 0,
    importance REAL NOT NULL DEFAULT 0,
    merged_into INTEGER REFERENCES entities(id),
    state TEXT NOT NULL DEFAULT 'live'
) STRICT;
CREATE TABLE entity_aliases(
    entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    alias_norm TEXT NOT NULL,
    source TEXT NOT NULL
) STRICT;
CREATE INDEX entity_aliases_norm ON entity_aliases(alias_norm);

CREATE TABLE facts(
    id INTEGER PRIMARY KEY,
    subject_id INTEGER NOT NULL REFERENCES entities(id),
    predicate TEXT NOT NULL,
    object_id INTEGER REFERENCES entities(id),
    object_value TEXT,
    valid_from INTEGER,
    valid_to INTEGER,
    recorded_at INTEGER NOT NULL,
    expired_at INTEGER,
    confidence REAL NOT NULL,
    importance REAL NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX facts_subject_live ON facts(subject_id, predicate) WHERE expired_at IS NULL;
CREATE INDEX facts_object_live ON facts(object_id) WHERE expired_at IS NULL;

CREATE TABLE mentions(
    entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    segment_id INTEGER NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    t INTEGER NOT NULL,
    span TEXT
) STRICT;

CREATE TABLE evidence(
    memory_kind TEXT NOT NULL,
    memory_id INTEGER NOT NULL,
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    snippet TEXT,
    weight REAL NOT NULL DEFAULT 1,
    t INTEGER NOT NULL
) STRICT;
CREATE INDEX evidence_memory ON evidence(memory_kind, memory_id);

CREATE TABLE jobs(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    priority INTEGER NOT NULL,
    deadline INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'queued',
    not_before INTEGER
) STRICT;

CREATE TABLE working_memory(
    id INTEGER PRIMARY KEY CHECK (id = 1),
    snapshot BLOB NOT NULL,
    t INTEGER NOT NULL
) STRICT;

CREATE TABLE audit(
    id INTEGER PRIMARY KEY,
    t INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    detail TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    hash TEXT NOT NULL
) STRICT;
"#;

/// Brings `connection` to the current schema.
///
/// # Errors
///
/// Fails when a migration does not apply, which leaves the database at the version before it.
pub(crate) fn migrate(connection: &Connection) -> Result<(), StoreError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let version = usize::try_from(version).unwrap_or(0);
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version) {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(
            None,
            "user_version",
            i64::try_from(index + 1).unwrap_or(i64::MAX),
        )?;
        transaction.commit()?;
    }
    Ok(())
}

pub(crate) const CURRENT_VERSION: usize = MIGRATIONS.len();
