use crate::db::{is_db_encrypted, open_db_readonly};
use crate::NmemError;
use std::path::Path;

pub fn handle_status(db_path: &Path) -> Result<(), NmemError> {
    if !db_path.exists() {
        eprintln!("nmem: no database at {}", db_path.display());
        return Ok(());
    }

    // File sizes
    let db_size = std::fs::metadata(db_path)?.len();
    let wal_path = db_path.with_extension("db-wal");
    let wal_size = std::fs::metadata(&wal_path).map(|m| m.len()).ok();

    let conn = open_db_readonly(db_path)?;

    // Counts
    let obs_count: i64 = conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))?;
    let prompt_count: i64 = conn.query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))?;
    let session_count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;

    // Observation type breakdown (top 5)
    let mut stmt = conn.prepare(
        "SELECT obs_type, COUNT(*) FROM observations GROUP BY obs_type ORDER BY COUNT(*) DESC LIMIT 5",
    )?;
    let type_breakdown: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()?;

    // Last session
    let last_session: Option<(i64, String)> = conn
        .query_row(
            "SELECT started_at, project FROM sessions ORDER BY started_at DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();

    // Print
    match wal_size {
        Some(ws) => eprintln!("nmem: database — {} (+{} WAL)", fmt_size(db_size), fmt_size(ws)),
        None => eprintln!("nmem: database — {}", fmt_size(db_size)),
    }

    if type_breakdown.is_empty() {
        eprintln!("nmem: observations — {obs_count}");
    } else {
        let parts: Vec<String> = type_breakdown
            .iter()
            .map(|(t, c)| format!("{t}: {c}"))
            .collect();
        eprintln!("nmem: observations — {obs_count} ({0})", parts.join(", "));
    }

    eprintln!("nmem: prompts — {prompt_count}");
    eprintln!("nmem: sessions — {session_count}");

    if let Some((ts, project)) = last_session {
        let date = format_epoch_date(ts);
        eprintln!("nmem: last session — {date} (project: {project})");
    }

    let encrypted = is_db_encrypted(db_path);
    eprintln!(
        "nmem: encryption — {}",
        if encrypted { "enabled" } else { "disabled" }
    );

    Ok(())
}

fn format_epoch_date(epoch_secs: i64) -> String {
    // Convert epoch seconds to YYYY-MM-DD
    // Days from unix epoch, then civil date
    let days = epoch_secs / 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert days since 1970-01-01 to (year, month, day).
/// Algorithm from Howard Hinnant's chrono-compatible date library.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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
