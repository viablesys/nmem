// Infrastructure (no prefix)
pub mod cli;
pub mod db;
pub mod metrics;
pub mod schema;
pub mod status;

// S1 Operations — capture, store, retrieve
pub mod s1_context;
pub mod s1_extract;
pub mod s1_pin;
pub mod s1_record;
pub mod s1_search;
pub mod s1_serve;

// S1's S4 — session intelligence (VSM recursion within S1)
pub mod s14_summarize;
pub mod s14_transcript;

// S3 Control — retention, compaction, integrity
pub mod s3_maintain;
pub mod s3_purge;
pub mod s3_sweep;

// S4 Intelligence — future work, cross-session planning
pub mod s4_dispatch;

// S5 Policy — config, boundaries, identity
pub mod s5_config;
pub mod s5_filter;
pub mod s5_project;

// Backward-compat aliases — external code (main.rs, tests) can use old names
pub use s1_context as context;
pub use s1_extract as extract;
pub use s1_pin as pin;
pub use s1_record as record;
pub use s1_search as search;
pub use s1_serve as serve;
pub use s14_summarize as summarize;
pub use s14_transcript as transcript;
pub use s3_maintain as maintain;
pub use s3_purge as purge;
pub use s3_sweep as sweep;
pub use s4_dispatch as dispatch;
pub use s5_config as config;
pub use s5_filter as filter;
pub use s5_project as project;

#[derive(Debug)]
pub enum NmemError {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
    Config(String),
}

impl std::fmt::Display for NmemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NmemError::Database(e) => write!(f, "database: {e}"),
            NmemError::Io(e) => write!(f, "io: {e}"),
            NmemError::Json(e) => write!(f, "json: {e}"),
            NmemError::Config(msg) => write!(f, "config: {msg}"),
        }
    }
}

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
