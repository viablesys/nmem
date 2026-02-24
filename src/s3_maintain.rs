use crate::cli::MaintainArgs;
use crate::s5_config::load_config;
use crate::s3_sweep::run_sweep;
use crate::db::open_db;
use crate::NmemError;
use std::path::Path;

pub fn handle_maintain(db_path: &Path, args: &MaintainArgs) -> Result<(), NmemError> {
    // Session-scoped maintenance: episodes → summarize → sweep → checkpoint
    if let Some(ref session_id) = args.session {
        return handle_session_maintain(db_path, session_id);
    }

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

    // Retention sweep
    if args.sweep {
        let config = load_config().unwrap_or_default();
        if !config.retention.enabled {
            eprintln!("nmem: retention sweep skipped (not enabled in config)");
        } else {
            let result = run_sweep(&conn, &config.retention)?;
            if result.deleted > 0 {
                for (obs_type, count) in &result.by_type {
                    eprintln!("nmem: sweep — {obs_type}: {count} deleted");
                }
                eprintln!("nmem: sweep — {} total deleted, {} orphans cleaned",
                    result.deleted, result.orphans_cleaned);
            } else {
                eprintln!("nmem: sweep — nothing to delete");
            }
        }
    }

    // Resummarize all sessions
    if args.resummarize {
        let config = load_config().unwrap_or_default();
        if !config.summarization.enabled {
            eprintln!("nmem: resummarize skipped (summarization not enabled)");
        } else {
            resummarize_all(&conn, &config.summarization)?;
        }
    }

    let size_after = std::fs::metadata(db_path)?.len();
    eprintln!("nmem: database: {} → {}", fmt_size(size_before), fmt_size(size_after));

    Ok(())
}

fn resummarize_all(
    conn: &rusqlite::Connection,
    config: &crate::s5_config::SummarizationConfig,
) -> Result<(), NmemError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM sessions WHERE summary IS NOT NULL ORDER BY started_at ASC",
    )?;
    let session_ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;

    let total = session_ids.len();
    eprintln!("nmem: resummarizing {total} sessions...");

    let mut success = 0u64;
    let mut failed = 0u64;
    for (i, sid) in session_ids.iter().enumerate() {
        match crate::s1_4_summarize::summarize_session(conn, sid, config) {
            Ok(()) => {
                success += 1;
                eprint!("\rnmem: [{}/{}] {} ok, {} failed", i + 1, total, success, failed);
            }
            Err(e) => {
                failed += 1;
                eprint!("\rnmem: [{}/{}] {} ok, {} failed", i + 1, total, success, failed);
                eprintln!(" — {sid}: {e}");
            }
        }
    }
    eprintln!("\nnmem: resummarize complete — {success} ok, {failed} failed");

    Ok(())
}

fn handle_session_maintain(db_path: &Path, session_id: &str) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;
    let config = load_config().unwrap_or_default();

    // Detect episodes — non-fatal
    match crate::s4_memory::detect_and_narrate_episodes(&conn, session_id, &config.summarization) {
        Ok(n) if n > 1 => eprintln!("nmem: {n} episodes detected"),
        Err(e) => eprintln!("nmem: episode detection failed (non-fatal): {e}"),
        _ => {}
    }

    // Summarize session — non-fatal
    match crate::s1_4_summarize::summarize_session(&conn, session_id, &config.summarization) {
        Ok(()) => eprintln!("nmem: session summarized"),
        Err(e) => eprintln!("nmem: summarization failed (non-fatal): {e}"),
    }

    // Retention sweep — non-fatal
    if config.retention.enabled {
        match run_sweep(&conn, &config.retention) {
            Ok(r) if r.deleted > 0 => {
                eprintln!("nmem: sweep deleted {} expired observations", r.deleted);
            }
            Err(e) => eprintln!("nmem: sweep error (non-fatal): {e}"),
            _ => {}
        }
    }

    // WAL checkpoint
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
        eprintln!("nmem: WAL checkpoint failed (non-fatal): {e}");
    }

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
