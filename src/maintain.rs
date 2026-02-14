use crate::cli::MaintainArgs;
use crate::db::open_db;
use crate::NmemError;
use std::path::Path;

pub fn handle_maintain(db_path: &Path, args: &MaintainArgs) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;

    let size_before = std::fs::metadata(db_path)?.len();

    // Incremental vacuum — reclaim freed pages
    let free_before: i64 = conn.pragma_query_value(None, "freelist_count", |r| r.get(0))?;
    conn.pragma_update(None, "incremental_vacuum", 0)?;
    let free_after: i64 = conn.pragma_query_value(None, "freelist_count", |r| r.get(0))?;
    let reclaimed = free_before - free_after;
    eprintln!("nmem: incremental vacuum — reclaimed {reclaimed} pages");

    // WAL checkpoint (TRUNCATE folds WAL into main file, then deletes WAL)
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    eprintln!("nmem: WAL checkpoint — ok");

    // FTS integrity check
    conn.execute_batch(
        "INSERT INTO observations_fts(observations_fts) VALUES('integrity-check')",
    )?;
    eprintln!("nmem: FTS integrity (observations) — ok");

    conn.execute_batch("INSERT INTO prompts_fts(prompts_fts) VALUES('integrity-check')")?;
    eprintln!("nmem: FTS integrity (prompts) — ok");

    // Optional FTS rebuild
    if args.rebuild_fts {
        conn.execute_batch(
            "INSERT INTO observations_fts(observations_fts) VALUES('rebuild')",
        )?;
        eprintln!("nmem: FTS rebuild (observations) — ok");

        conn.execute_batch("INSERT INTO prompts_fts(prompts_fts) VALUES('rebuild')")?;
        eprintln!("nmem: FTS rebuild (prompts) — ok");
    }

    let size_after = std::fs::metadata(db_path)?.len();
    eprintln!("nmem: database: {} → {}", fmt_size(size_before), fmt_size(size_after));

    Ok(())
}

fn fmt_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
