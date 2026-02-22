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
    /// Search observations by full-text query
    Search(SearchArgs),
    /// Encrypt the database (migrate from unencrypted to SQLCipher)
    Encrypt,
    /// Pin an observation (exempt from retention sweeps)
    Pin(PinArgs),
    /// Unpin an observation (restore to normal retention)
    Unpin(PinArgs),
    /// Show what nmem would inject at session start
    Context(ContextArgs),
    /// Queue a task for later execution
    Queue(QueueArgs),
    /// Check for pending tasks and dispatch to tmux
    Dispatch(DispatchArgs),
    /// View a task's status and output
    Task(TaskArgs),
    /// Detect cross-session patterns and write learnings report
    Learn(LearnArgs),
    /// Backfill classifier labels for observations with NULL values
    Backfill(BackfillArgs),
}

#[derive(Parser)]
pub struct BackfillArgs {
    /// Dimension to backfill: phase, scope, locus, novelty, friction (default: phase)
    #[arg(long, default_value = "phase")]
    pub dimension: String,

    /// Batch size for commits (default 500)
    #[arg(long, default_value = "500")]
    pub batch_size: usize,

    /// Dry run — show counts but don't update
    #[arg(long)]
    pub dry_run: bool,

    /// Training corpus size (recorded in classifier_runs)
    #[arg(long)]
    pub corpus_size: Option<i64>,

    /// Cross-validation accuracy (recorded in classifier_runs)
    #[arg(long)]
    pub cv_accuracy: Option<f64>,

    /// Extra notes for the classifier run metadata JSON
    #[arg(long)]
    pub notes: Option<String>,
}

impl BackfillArgs {
    /// Build optional metadata JSON from CLI args.
    pub fn metadata_json(&self) -> Option<String> {
        let mut obj = serde_json::Map::new();
        obj.insert("source".into(), serde_json::Value::String("backfill".into()));
        if let Some(notes) = &self.notes {
            obj.insert("notes".into(), serde_json::Value::String(notes.clone()));
        }
        Some(serde_json::to_string(&serde_json::Value::Object(obj)).unwrap())
    }
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
pub struct SearchArgs {
    /// FTS5 search query (supports AND/OR/NOT, "phrases", prefix*)
    pub query: String,

    /// Filter by project name
    #[arg(long)]
    pub project: Option<String>,

    /// Filter by observation type (e.g. file_read, command, file_edit)
    #[arg(long = "type")]
    pub obs_type: Option<String>,

    /// Maximum results (default 20, max 100)
    #[arg(long, default_value = "20")]
    pub limit: i64,

    /// Fetch full observation details (not just index)
    #[arg(long)]
    pub full: bool,

    /// Output observation IDs only (one per line)
    #[arg(long)]
    pub ids: bool,

    /// Ranking order: "relevance" (BM25 only) or "blended" (BM25 + recency + type weight)
    #[arg(long, default_value = "relevance")]
    pub order_by: String,
}

#[derive(Parser)]
pub struct PinArgs {
    /// Observation ID
    pub id: i64,
}

#[derive(Parser)]
pub struct ContextArgs {
    /// Project name (defaults to current directory)
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Parser)]
pub struct MaintainArgs {
    /// Also rebuild FTS5 indexes (rewrites entire index)
    #[arg(long)]
    pub rebuild_fts: bool,

    /// Run retention sweep (deletes expired observations per config)
    #[arg(long)]
    pub sweep: bool,
}

#[derive(Parser)]
pub struct QueueArgs {
    /// The task prompt
    pub prompt: String,

    /// Project scope (defaults to cwd-derived)
    #[arg(long)]
    pub project: Option<String>,

    /// Working directory (defaults to current)
    #[arg(long)]
    pub cwd: Option<String>,

    /// When to run: "5m", "2h", "1d", "tomorrow", "tonight", or ISO datetime
    #[arg(long)]
    pub after: String,
}

#[derive(Parser)]
pub struct DispatchArgs {
    /// Task file to queue and dispatch immediately
    pub file: Option<PathBuf>,

    /// Maximum concurrent running tasks (default 1)
    #[arg(long, default_value = "1")]
    pub max_concurrent: u32,

    /// Show what would be dispatched without doing it
    #[arg(long)]
    pub dry_run: bool,

    /// tmux session name (default "nmem")
    #[arg(long, default_value = "nmem")]
    pub tmux_session: String,
}

#[derive(Parser)]
pub struct TaskArgs {
    /// Task ID
    pub id: i64,

    /// Show output only (for piping)
    #[arg(long)]
    pub output: bool,
}

#[derive(Parser)]
pub struct LearnArgs {
    /// Output file (default: ~/.nmem/learnings.md)
    #[arg(long, short)]
    pub output: Option<PathBuf>,

    /// Minimum sessions for a pattern to qualify (default: 3)
    #[arg(long, default_value = "3")]
    pub threshold: i64,

    /// Half-life in hours for heat decay (default: 168 = 1 week)
    #[arg(long, default_value = "168")]
    pub half_life: f64,
}
