use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nmem", version, about = "Cross-session memory for Claude Code")]
pub struct Cli {
    /// Database path
    #[arg(long, env = "NMEM_DB", global = true)]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Record a hook event from stdin
    Record,
    /// Start MCP query server on stdio
    Serve,
    /// Purge observations, prompts, and sessions
    Purge(PurgeArgs),
    /// Run database maintenance (vacuum, WAL checkpoint, FTS integrity)
    Maintain(MaintainArgs),
    /// Show database health: size, counts, last session
    Status,
    /// Encrypt the database (migrate from unencrypted to SQLCipher)
    Encrypt,
}

#[derive(Parser)]
pub struct PurgeArgs {
    /// Delete observations before this date (YYYY-MM-DD)
    #[arg(long)]
    pub before: Option<String>,

    /// Delete everything for a project
    #[arg(long)]
    pub project: Option<String>,

    /// Delete everything for a session ID
    #[arg(long)]
    pub session: Option<String>,

    /// Delete a single observation by ID
    #[arg(long)]
    pub id: Option<i64>,

    /// Delete observations of this type (e.g. file_read, command)
    #[arg(long = "type")]
    pub obs_type: Option<String>,

    /// Used with --type: delete observations older than N days
    #[arg(long, requires = "obs_type")]
    pub older_than: Option<u32>,

    /// Delete observations matching FTS query
    #[arg(long)]
    pub search: Option<String>,

    /// Skip confirmation — actually delete
    #[arg(long)]
    pub confirm: bool,
}

#[derive(Parser)]
pub struct MaintainArgs {
    /// Also rebuild FTS5 indexes (rewrites entire index)
    #[arg(long)]
    pub rebuild_fts: bool,
}
