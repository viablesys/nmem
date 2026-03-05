use crate::cli::PurgeArgs;
use crate::db::open_db;
use crate::NmemError;
use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

struct PurgeCounts {
    observations: usize,
    prompts: usize,
    sessions: usize,
}

fn parse_date_to_ts(date: &str) -> Result<i64, NmemError> {
    // Expect YYYY-MM-DD, convert to start-of-day UTC
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(NmemError::Config(format!("invalid date: {date} (expected YYYY-MM-DD)")));
    }
    let y: i64 = parts[0].parse().map_err(|_| NmemError::Config(format!("invalid year: {}", parts[0])))?;
    let m: i64 = parts[1].parse().map_err(|_| NmemError::Config(format!("invalid month: {}", parts[1])))?;
    let d: i64 = parts[2].parse().map_err(|_| NmemError::Config(format!("invalid day: {}", parts[2])))?;

    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(NmemError::Config(format!("invalid date: {date}")));
    }

    // Validate days per month
    let is_leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max_day: i64 = match m {
        2 => if is_leap { 29 } else { 28 },
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d > max_day {
        return Err(NmemError::Config(format!("invalid date: {date} (month {m} has {max_day} days)")));
    }

    // Simple days-since-epoch calculation (good enough for purge cutoffs)
    let days = (y - 1970) * 365 + (y - 1969) / 4 - (y - 1901) / 100 + (y - 1601) / 400;
    let month_days = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut day_of_year = month_days[m as usize - 1] + d;
    if m > 2 && (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) {
        day_of_year += 1;
    }

    Ok((days + day_of_year - 1) * 86400)
}

fn days_ago_ts(days: u32) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // +1 so --older-than 0 includes records written this second
    now + 1 - (days as i64 * 86400)
}

fn has_any_filter(args: &PurgeArgs) -> bool {
    args.before.is_some()
        || args.project.is_some()
        || args.session.is_some()
        || args.id.is_some()
        || args.obs_type.is_some()
        || args.search.is_some()
}

/// Count matching observations, prompts, and sessions that would be purged.
fn count_targets(conn: &Connection, args: &PurgeArgs) -> Result<PurgeCounts, NmemError> {
    let obs_count = count_observations(conn, args)?;
    let prompt_count = count_prompts(conn, args)?;
    let session_count = count_sessions(conn, args)?;
    Ok(PurgeCounts {
        observations: obs_count,
        prompts: prompt_count,
        sessions: session_count,
    })
}

fn count_observations(conn: &Connection, args: &PurgeArgs) -> Result<usize, NmemError> {
    let (where_clause, bind_values) = build_obs_where(args)?;
    let sql = format!("SELECT COUNT(*) FROM observations WHERE {where_clause}");
    let count: i64 = conn.query_row(&sql, rusqlite::params_from_iter(&bind_values), |r| r.get(0))?;
    Ok(count as usize)
}

fn count_prompts(conn: &Connection, args: &PurgeArgs) -> Result<usize, NmemError> {
    // Only session/project/before purge prompts directly
    if let Some(ref session) = args.session {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM prompts WHERE session_id = ?1",
            params![session],
            |r| r.get(0),
        )?;
        return Ok(count as usize);
    }
    if let Some(ref project) = args.project {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM prompts WHERE session_id IN (SELECT id FROM sessions WHERE project = ?1)",
            params![project],
            |r| r.get(0),
        )?;
        return Ok(count as usize);
    }
    if let Some(ref before) = args.before {
        let ts = parse_date_to_ts(before)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM prompts WHERE timestamp < ?1",
            params![ts],
            |r| r.get(0),
        )?;
        return Ok(count as usize);
    }
    Ok(0)
}

fn count_sessions(conn: &Connection, args: &PurgeArgs) -> Result<usize, NmemError> {
    if let Some(ref session) = args.session {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            params![session],
            |r| r.get(0),
        )?;
        return Ok(count as usize);
    }
    if let Some(ref project) = args.project {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        return Ok(count as usize);
    }
    // For other modes, we'd only delete orphans — hard to pre-count exactly
    Ok(0)
}

/// Build WHERE clause for observations based on args. Returns (clause, bind_values).
fn build_obs_where(args: &PurgeArgs) -> Result<(String, Vec<String>), NmemError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();

    if let Some(id) = args.id {
        clauses.push(format!("id = ?{}", values.len() + 1));
        values.push(id.to_string());
    }

    if let Some(ref session) = args.session {
        clauses.push(format!("session_id = ?{}", values.len() + 1));
        values.push(session.clone());
    }

    if let Some(ref project) = args.project {
        clauses.push(format!(
            "session_id IN (SELECT id FROM sessions WHERE project = ?{})",
            values.len() + 1
        ));
        values.push(project.clone());
    }

    if let Some(ref before) = args.before {
        let ts = parse_date_to_ts(before)?;
        clauses.push(format!("timestamp < ?{}", values.len() + 1));
        values.push(ts.to_string());
    }

    if let Some(ref obs_type) = args.obs_type {
        clauses.push(format!("obs_type = ?{}", values.len() + 1));
        values.push(obs_type.clone());

        if let Some(older_than) = args.older_than {
            let cutoff = days_ago_ts(older_than);
            clauses.push(format!("timestamp < ?{}", values.len() + 1));
            values.push(cutoff.to_string());
        }
    }

    if let Some(ref search) = args.search {
        let sanitized = crate::sanitize_fts_query(search)
            .ok_or_else(|| NmemError::Config("search query produced no usable terms".into()))?;
        clauses.push(format!(
            "id IN (SELECT rowid FROM observations_fts WHERE observations_fts MATCH ?{})",
            values.len() + 1
        ));
        values.push(sanitized);
    }

    if clauses.is_empty() {
        return Err(NmemError::Config("at least one filter flag is required".into()));
    }

    Ok((clauses.join(" AND "), values))
}

fn delete_observations(conn: &Connection, args: &PurgeArgs) -> Result<usize, NmemError> {
    let (where_clause, bind_values) = build_obs_where(args)?;
    let sql = format!("DELETE FROM observations WHERE {where_clause}");
    let deleted = conn.execute(&sql, rusqlite::params_from_iter(&bind_values))?;
    Ok(deleted)
}

fn delete_prompts_for_session(conn: &Connection, session_id: &str) -> Result<usize, NmemError> {
    let deleted = conn.execute("DELETE FROM prompts WHERE session_id = ?1", params![session_id])?;
    Ok(deleted)
}

fn delete_prompts_for_project(conn: &Connection, project: &str) -> Result<usize, NmemError> {
    let deleted = conn.execute(
        "DELETE FROM prompts WHERE session_id IN (SELECT id FROM sessions WHERE project = ?1)",
        params![project],
    )?;
    Ok(deleted)
}

fn delete_prompts_before(conn: &Connection, ts: i64) -> Result<usize, NmemError> {
    let deleted = conn.execute("DELETE FROM prompts WHERE timestamp < ?1", params![ts])?;
    Ok(deleted)
}

fn delete_session(conn: &Connection, session_id: &str) -> Result<usize, NmemError> {
    conn.execute("DELETE FROM observations WHERE session_id = ?1", params![session_id])?;
    conn.execute("DELETE FROM prompts WHERE session_id = ?1", params![session_id])?;
    conn.execute("DELETE FROM work_units WHERE session_id = ?1", params![session_id])?;
    conn.execute("DELETE FROM _cursor WHERE session_id = ?1", params![session_id])?;
    let deleted = conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    Ok(deleted)
}

fn delete_sessions_for_project(conn: &Connection, project: &str) -> Result<usize, NmemError> {
    conn.execute(
        "DELETE FROM observations WHERE session_id IN (SELECT id FROM sessions WHERE project = ?1)",
        params![project],
    )?;
    conn.execute(
        "DELETE FROM prompts WHERE session_id IN (SELECT id FROM sessions WHERE project = ?1)",
        params![project],
    )?;
    conn.execute(
        "DELETE FROM work_units WHERE session_id IN (SELECT id FROM sessions WHERE project = ?1)",
        params![project],
    )?;
    conn.execute(
        "DELETE FROM _cursor WHERE session_id IN (SELECT id FROM sessions WHERE project = ?1)",
        params![project],
    )?;
    let deleted = conn.execute("DELETE FROM sessions WHERE project = ?1", params![project])?;
    Ok(deleted)
}

pub fn cleanup_orphans(conn: &Connection) -> Result<usize, NmemError> {
    // Delete leaf rows referencing missing sessions
    conn.execute_batch("DELETE FROM observations WHERE session_id NOT IN (SELECT id FROM sessions)")?;
    conn.execute_batch("DELETE FROM prompts WHERE session_id NOT IN (SELECT id FROM sessions)")?;
    conn.execute_batch("DELETE FROM work_units WHERE session_id NOT IN (SELECT id FROM sessions)")?;
    conn.execute_batch("DELETE FROM _cursor WHERE session_id NOT IN (SELECT id FROM sessions)")?;
    // Delete sessions that have no observations or prompts left
    let orphaned = conn.execute(
        "DELETE FROM sessions WHERE id NOT IN (
            SELECT DISTINCT session_id FROM observations
            UNION
            SELECT DISTINCT session_id FROM prompts
        )",
        [],
    )?;
    Ok(orphaned)
}

pub fn post_purge_maintenance(conn: &Connection, obs_deleted: usize) -> Result<(), NmemError> {
    conn.pragma_update(None, "incremental_vacuum", 0)?;

    if obs_deleted > 1000 {
        conn.execute_batch("INSERT INTO observations_fts(observations_fts) VALUES('rebuild')")?;
    }

    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(())
}

pub fn handle_purge(db_path: &Path, args: &PurgeArgs) -> Result<(), NmemError> {
    if !has_any_filter(args) {
        return Err(NmemError::Config(
            "at least one filter flag is required (--before, --project, --session, --id, --type, --search)".into(),
        ));
    }

    let conn = open_db(db_path)?;
    conn.pragma_update(None, "secure_delete", "ON")?;

    let counts = count_targets(&conn, args)?;
    let total = counts.observations + counts.prompts + counts.sessions;

    if total == 0 {
        eprintln!("nmem: nothing to purge");
        return Ok(());
    }

    eprintln!(
        "nmem: would purge {} observations, {} prompts, {} sessions",
        counts.observations, counts.prompts, counts.sessions
    );

    if !args.confirm {
        eprintln!("nmem: re-run with --confirm to delete");
        return Ok(());
    }

    // Execute deletion inside a transaction
    let tx = conn.unchecked_transaction()?;

    // 1. Delete observations (leaf)
    let obs_deleted = delete_observations(&tx, args)?;

    // 2. Delete prompts for session/project/before modes
    let mut prompts_deleted = 0;
    if let Some(ref session) = args.session {
        prompts_deleted += delete_prompts_for_session(&tx, session)?;
    }
    if let Some(ref project) = args.project {
        prompts_deleted += delete_prompts_for_project(&tx, project)?;
    }
    if let Some(ref before) = args.before {
        let ts = parse_date_to_ts(before)?;
        prompts_deleted += delete_prompts_before(&tx, ts)?;
    }

    // 3. Delete sessions for session/project modes
    let mut sessions_deleted = 0;
    if let Some(ref session) = args.session {
        sessions_deleted += delete_session(&tx, session)?;
    } else if let Some(ref project) = args.project {
        sessions_deleted += delete_sessions_for_project(&tx, project)?;
    }

    // 4. Cleanup orphans for other modes
    if args.session.is_none() && args.project.is_none() {
        sessions_deleted += cleanup_orphans(&tx)?;
    }

    tx.commit()?;

    // Post-deletion maintenance (outside transaction)
    post_purge_maintenance(&conn, obs_deleted)?;

    eprintln!(
        "nmem: purged {} observations, {} prompts, {} sessions",
        obs_deleted, prompts_deleted, sessions_deleted
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }

    fn insert_session(conn: &Connection, id: &str, project: &str) {
        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES (?1, ?2, ?3)",
            params![id, project, 1700000000],
        )
        .unwrap();
    }

    fn insert_observation(conn: &Connection, session_id: &str, content: &str) {
        conn.execute(
            "INSERT INTO observations (session_id, timestamp, obs_type, source_event, content)
             VALUES (?1, ?2, 'command', 'PostToolUse', ?3)",
            params![session_id, 1700000000, content],
        )
        .unwrap();
    }

    fn insert_prompt(conn: &Connection, session_id: &str, content: &str) {
        conn.execute(
            "INSERT INTO prompts (session_id, timestamp, source, content)
             VALUES (?1, ?2, 'human', ?3)",
            params![session_id, 1700000000, content],
        )
        .unwrap();
    }

    #[test]
    fn parse_date_known_epoch() {
        // 2025-01-01 should be 1735689600
        let ts = parse_date_to_ts("2025-01-01").unwrap();
        assert_eq!(ts, 1735689600);
    }

    #[test]
    fn parse_date_invalid() {
        assert!(parse_date_to_ts("not-a-date").is_err());
        assert!(parse_date_to_ts("2025-13-01").is_err());
        assert!(parse_date_to_ts("2025-01-32").is_err());
    }

    #[test]
    fn parse_date_rejects_invalid_day_for_month() {
        // April has 30 days, not 31
        assert!(parse_date_to_ts("2025-04-31").is_err());
        // February never has 30 days
        assert!(parse_date_to_ts("2025-02-30").is_err());
        // February 29 in a non-leap year
        assert!(parse_date_to_ts("2025-02-29").is_err());
        // February 29 in a leap year should succeed
        assert!(parse_date_to_ts("2024-02-29").is_ok());
    }

    #[test]
    fn delete_session_with_observations_and_prompts() {
        let conn = setup_test_db();
        insert_session(&conn, "sess-1", "test-project");
        insert_observation(&conn, "sess-1", "cargo build");
        insert_prompt(&conn, "sess-1", "build the project");

        // Should succeed — must delete observations and prompts before session
        let result = delete_session(&conn, "sess-1");
        assert!(result.is_ok(), "delete_session failed: {result:?}");

        // Verify everything is gone
        let obs_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE session_id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(obs_count, 0);

        let prompt_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM prompts WHERE session_id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(prompt_count, 0);
    }

    #[test]
    fn delete_sessions_for_project_with_dependents() {
        let conn = setup_test_db();
        insert_session(&conn, "sess-a", "proj-x");
        insert_session(&conn, "sess-b", "proj-x");
        insert_observation(&conn, "sess-a", "test obs a");
        insert_observation(&conn, "sess-b", "test obs b");
        insert_prompt(&conn, "sess-a", "prompt a");

        let result = delete_sessions_for_project(&conn, "proj-x");
        assert!(result.is_ok(), "delete_sessions_for_project failed: {result:?}");

        let obs_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(obs_count, 0);

        let prompt_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(prompt_count, 0);
    }

    #[test]
    fn purge_id_is_parameterized() {
        // If build_obs_where uses format!("id = {id}") instead of a parameter,
        // a crafted id could inject SQL. Verify it uses proper parameterization
        // by checking that the generated clause uses a bind parameter.
        let args = PurgeArgs {
            id: Some(42),
            before: None,
            project: None,
            session: None,
            obs_type: None,
            older_than: None,
            search: None,
            confirm: false,
        };
        let (clause, values) = build_obs_where(&args).unwrap();
        // Should use a bind parameter like "id = ?1", not "id = 42"
        assert!(
            clause.contains('?'),
            "id filter should use parameterized query, got: {clause}"
        );
        assert!(
            !clause.contains("42"),
            "id value should not be interpolated into SQL, got: {clause}"
        );
        assert_eq!(values.len(), 1, "should have one bind value");
    }

    #[test]
    fn cleanup_orphans_removes_dangling_observations() {
        let conn = setup_test_db();
        insert_session(&conn, "sess-1", "test");
        insert_observation(&conn, "sess-1", "test content");

        // Disable FK checks temporarily to simulate orphaned state
        conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
        conn.execute("DELETE FROM sessions WHERE id = 'sess-1'", [])
            .unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();

        // Now we have an observation with no parent session
        let orphan_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphan_count, 1);

        cleanup_orphans(&conn).unwrap();

        // Orphaned observations should be cleaned up
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "orphaned observations should be deleted");
    }
}
