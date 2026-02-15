pub mod cli;
pub mod config;
pub mod context;
pub mod db;
pub mod extract;
pub mod filter;
pub mod maintain;
pub mod metrics;
pub mod pin;
pub mod project;
pub mod purge;
pub mod record;
pub mod schema;
pub mod search;
pub mod serve;
pub mod summarize;
pub mod sweep;
pub mod status;
pub mod transcript;

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
