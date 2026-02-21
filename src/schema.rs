use rusqlite_migration::{M, Migrations};
use std::sync::LazyLock;

pub static MIGRATIONS: LazyLock<Migrations<'static>> = LazyLock::new(|| {
    Migrations::new(vec![
        M::up(
            "
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    signature   TEXT,
    summary     TEXT
);

CREATE TABLE prompts (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    timestamp   INTEGER NOT NULL,
    source      TEXT NOT NULL,
    content     TEXT NOT NULL
);

CREATE TABLE observations (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    prompt_id   INTEGER REFERENCES prompts(id),
    timestamp   INTEGER NOT NULL,
    obs_type    TEXT NOT NULL,
    source_event TEXT NOT NULL,
    tool_name   TEXT,
    file_path   TEXT,
    content     TEXT NOT NULL,
    metadata    TEXT
);

CREATE TABLE _cursor (
    session_id  TEXT PRIMARY KEY,
    line_number INTEGER NOT NULL DEFAULT 0
);

-- Indexes
CREATE INDEX idx_obs_dedup ON observations(session_id, obs_type, file_path, timestamp);
CREATE INDEX idx_obs_session ON observations(session_id, timestamp);
CREATE INDEX idx_obs_prompt ON observations(prompt_id);
CREATE INDEX idx_obs_type ON observations(obs_type);
CREATE INDEX idx_obs_file ON observations(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX idx_prompts_session ON prompts(session_id, id);

-- FTS5 for observations
CREATE VIRTUAL TABLE observations_fts USING fts5(
    content, content='observations', content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER observations_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER observations_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER observations_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;

-- FTS5 for prompts
CREATE VIRTUAL TABLE prompts_fts USING fts5(
    content, content='prompts', content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER prompts_ai AFTER INSERT ON prompts BEGIN
    INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER prompts_ad AFTER DELETE ON prompts BEGIN
    INSERT INTO prompts_fts(prompts_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;
",
        ),
        M::up(
            "ALTER TABLE observations ADD COLUMN is_pinned INTEGER NOT NULL DEFAULT 0;",
        ),
        M::up(
            "
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY,
    created_at   INTEGER NOT NULL DEFAULT (unixepoch('now')),
    status       TEXT NOT NULL DEFAULT 'pending',
    prompt       TEXT NOT NULL,
    project      TEXT,
    cwd          TEXT,
    tmux_target  TEXT,
    started_at   INTEGER,
    completed_at INTEGER,
    error        TEXT
);
CREATE INDEX idx_tasks_status ON tasks(status, created_at);
",
        ),
        M::up("ALTER TABLE tasks ADD COLUMN run_after INTEGER;"),
        M::up("ALTER TABLE tasks ADD COLUMN output_path TEXT;"),
        M::up(
            "
CREATE TABLE work_units (
    id              INTEGER PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id),
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER,
    intent          TEXT,
    first_prompt_id INTEGER,
    last_prompt_id  INTEGER,
    hot_files       TEXT,
    phase_signature TEXT,
    obs_count       INTEGER,
    summary         TEXT,
    learned         TEXT,
    notes           TEXT
);
CREATE INDEX idx_wu_session ON work_units(session_id);
",
        ),
        M::up("ALTER TABLE observations ADD COLUMN phase TEXT;"),
        M::up(
            "
CREATE TABLE classifier_runs (
    id          INTEGER PRIMARY KEY,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch('now')),
    name        TEXT NOT NULL,
    model_hash  TEXT NOT NULL,
    corpus_size INTEGER,
    cv_accuracy REAL,
    metadata    TEXT
);
CREATE INDEX idx_cr_name ON classifier_runs(name, created_at);

ALTER TABLE observations ADD COLUMN classifier_run_id INTEGER REFERENCES classifier_runs(id);
",
        ),
        M::up(
            "
ALTER TABLE observations ADD COLUMN scope TEXT;
ALTER TABLE observations ADD COLUMN scope_run_id INTEGER REFERENCES classifier_runs(id);
",
        ),
    ])
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_valid() {
        assert!(MIGRATIONS.validate().is_ok());
    }

    #[test]
    fn migrations_apply_to_memory_db() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(tables.contains(&"sessions".into()));
        assert!(tables.contains(&"prompts".into()));
        assert!(tables.contains(&"observations".into()));
        assert!(tables.contains(&"_cursor".into()));

        // Verify triggers exist
        let triggers: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(triggers.contains(&"observations_ai".into()));
        assert!(triggers.contains(&"observations_ad".into()));
        assert!(triggers.contains(&"prompts_ai".into()));
        assert!(triggers.contains(&"prompts_ad".into()));
    }
}
