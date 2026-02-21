use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

use nmem::cli::{Cli, Command};
use nmem::NmemError;

fn default_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".nmem").join("nmem.db")
}

fn run() -> Result<(), NmemError> {
    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(default_db_path);

    match cli.command {
        Command::Record => nmem::record::handle_record(&db_path),
        Command::Serve => nmem::serve::handle_serve(&db_path),
        Command::Purge(args) => nmem::purge::handle_purge(&db_path, &args),
        Command::Maintain(args) => nmem::maintain::handle_maintain(&db_path, &args),
        Command::Status => nmem::status::handle_status(&db_path),
        Command::Search(args) => nmem::search::handle_search(&db_path, &args),
        Command::Encrypt => nmem::db::handle_encrypt(&db_path),
        Command::Pin(args) => nmem::pin::handle_pin(&db_path, args.id),
        Command::Unpin(args) => nmem::pin::handle_unpin(&db_path, args.id),
        Command::Context(args) => nmem::context::handle_context(&db_path, &args),
        Command::Queue(args) => nmem::dispatch::handle_queue(&db_path, &args),
        Command::Dispatch(args) => nmem::dispatch::handle_dispatch(&db_path, &args),
        Command::Task(args) => nmem::dispatch::handle_task(&db_path, &args),
        Command::Learn(args) => nmem::learn::handle_learn(&db_path, &args),
        Command::Backfill(args) => nmem::s2_classify::handle_backfill(&db_path, &args),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nmem: {e}");
            ExitCode::from(1)
        }
    }
}
