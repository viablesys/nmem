// Infrastructure (no prefix)
pub mod cli;
pub mod db;
pub mod metrics;
pub mod query;
pub mod schema;
pub mod status;

// S1 Operations — capture, store, retrieve
pub mod s1_extract;
pub mod s1_git;
pub mod s1_lsp;
pub mod s1_mark;
pub mod s1_pin;
pub mod s1_record;
pub mod s1_search;
pub mod s1_serve;

// S2 Coordination — classification, dedup
pub mod s2_classify;
pub mod s2_inference;
pub mod s2_locus;
pub mod s2_novelty;
pub mod s2_scope;

// S1's S4 — session intelligence (VSM recursion within S1)
pub mod s1_4_inference;
pub mod s1_4_summarize;
pub mod s1_4_transcript;

// S3 Control — retention, compaction, integrity
pub mod s3_learn;
pub mod s3_maintain;
pub mod s3_purge;
pub mod s3_sweep;

// S4 Intelligence — context injection, task dispatch, cross-session patterns, episodic memory, fleet beacon
pub mod s4_beacon;
pub mod s4_context;
pub mod s4_dispatch;
pub mod s4_memory;

// S5 Policy — config, boundaries, identity
pub mod s5_config;
pub mod s5_filter;
pub mod s5_project;

// Backward-compat aliases — external code (main.rs, tests) can use old names
pub use s4_context as context;
pub use s1_extract as extract;
pub use s1_mark as mark;
pub use s1_pin as pin;
pub use s1_record as record;
pub use s1_search as search;
pub use s1_serve as serve;
pub use s1_4_summarize as summarize;
pub use s1_4_transcript as transcript;
pub use s3_learn as learn;
pub use s3_maintain as maintain;
pub use s3_purge as purge;
pub use s3_sweep as sweep;
pub use s4_dispatch as dispatch;
pub use s4_memory as memory;
pub use s5_config as config;
pub use s5_filter as filter;
pub use s5_project as project;

#[derive(Debug)]
pub enum NmemError {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Config(String),
    Nats(String),
}

impl std::fmt::Display for NmemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NmemError::Database(e) => write!(f, "database: {e}"),
            NmemError::Io(e) => write!(f, "io: {e}"),
            NmemError::Json(e) => write!(f, "json: {e}"),
            NmemError::Config(msg) => write!(f, "config: {msg}"),
            NmemError::Nats(msg) => write!(f, "nats: {msg}"),
        }
    }
}

impl std::error::Error for NmemError {}

impl From<rusqlite::Error> for NmemError {
    fn from(e: rusqlite::Error) -> Self {
        NmemError::Database(e)
    }
}

impl From<std::io::Error> for NmemError {
    fn from(e: std::io::Error) -> Self {
        NmemError::Io(e)
    }
}

impl From<serde_json::Error> for NmemError {
    fn from(e: serde_json::Error) -> Self {
        NmemError::Json(e)
    }
}

impl From<rusqlite_migration::Error> for NmemError {
    fn from(e: rusqlite_migration::Error) -> Self {
        match e {
            rusqlite_migration::Error::RusqliteError { query: _, err } => NmemError::Database(err),
            other => NmemError::Config(format!("migration: {other}")),
        }
    }
}

pub fn schema_migrations() -> &'static rusqlite_migration::Migrations<'static> {
    &schema::MIGRATIONS
}

/// Returns the directory containing the nmem binary.
///
/// This is the canonical root for all nmem data: DB, key, config, models, logs,
/// and future artifacts (fleet). All default paths are derived from here, making
/// nmem self-contained and relocatable without depending on HOME or USERPROFILE.
pub fn install_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

// Re-export query functions for backward compatibility
pub use query::sanitize_fts_query;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_is_parent_of_current_exe() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(install_dir(), exe.parent().unwrap());
    }

    #[test]
    fn install_dir_is_absolute() {
        assert!(install_dir().is_absolute());
    }

    #[test]
    fn install_dir_exists() {
        assert!(install_dir().is_dir());
    }
}
