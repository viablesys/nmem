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
}
