use crate::config::RetentionConfig;
use crate::purge::{cleanup_orphans, post_purge_maintenance};
use crate::NmemError;
use rusqlite::{Connection, params};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SweepResult {
    pub deleted: usize,
    pub by_type: Vec<(String, usize)>,
    pub orphans_cleaned: usize,
}

fn has_syntheses_table(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='syntheses'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

pub fn run_sweep(conn: &Connection, config: &RetentionConfig) -> Result<SweepResult, NmemError> {
    if !config.enabled {
        return Ok(SweepResult {
            deleted: 0,
            by_type: Vec::new(),
            orphans_cleaned: 0,
        });
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let has_syntheses = has_syntheses_table(conn);
    let tx = conn.unchecked_transaction()?;

    let mut total_deleted = 0usize;
    let mut by_type = Vec::new();

    for (obs_type, days) in &config.days {
        let cutoff = now - (*days as i64 * 86400);

        let deleted = if has_syntheses {
            tx.execute(
                "DELETE FROM observations WHERE obs_type = ?1 AND timestamp < ?2
                 AND id NOT IN (SELECT value FROM syntheses, json_each(syntheses.source_obs_ids))",
                params![obs_type, cutoff],
            )?
        } else {
            tx.execute(
                "DELETE FROM observations WHERE obs_type = ?1 AND timestamp < ?2",
                params![obs_type, cutoff],
            )?
        };

        if deleted > 0 {
            by_type.push((obs_type.clone(), deleted));
            total_deleted += deleted;
        }
    }

    let orphans_cleaned = cleanup_orphans(&tx)?;
    tx.commit()?;

    post_purge_maintenance(conn, total_deleted)?;

    Ok(SweepResult {
        deleted: total_deleted,
        by_type,
        orphans_cleaned,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn setup_db() -> (TempDir, Connection) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = open_db(&db_path).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prompts (session_id, timestamp, source, content) VALUES ('s1', 1000, 'user', 'hello')",
            [],
        )
        .unwrap();
        (dir, conn)
    }

    fn insert_obs(conn: &Connection, obs_type: &str, timestamp: i64) {
        conn.execute(
            "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content)
             VALUES ('s1', 1, ?1, ?2, 'PostToolUse', 'test')",
            params![timestamp, obs_type],
        )
        .unwrap();
    }

    #[test]
    fn sweep_disabled_is_noop() {
        let (_dir, conn) = setup_db();
        insert_obs(&conn, "file_read", 1000);

        let config = RetentionConfig {
            enabled: false,
            days: HashMap::from([("file_read".into(), 1)]),
        };

        let result = run_sweep(&conn, &config).unwrap();
        assert_eq!(result.deleted, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn sweep_deletes_expired() {
        let (_dir, conn) = setup_db();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Old observation (200 days ago)
        insert_obs(&conn, "file_read", now - 200 * 86400);
        // Recent observation (1 day ago)
        insert_obs(&conn, "file_read", now - 86400);

        let config = RetentionConfig {
            enabled: true,
            days: HashMap::from([("file_read".into(), 90)]),
        };

        let result = run_sweep(&conn, &config).unwrap();
        assert_eq!(result.deleted, 1);
        assert_eq!(result.by_type.len(), 1);
        assert_eq!(result.by_type[0].0, "file_read");
        assert_eq!(result.by_type[0].1, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn sweep_preserves_unexpired() {
        let (_dir, conn) = setup_db();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        insert_obs(&conn, "file_read", now - 10 * 86400);
        insert_obs(&conn, "file_read", now - 5 * 86400);

        let config = RetentionConfig {
            enabled: true,
            days: HashMap::from([("file_read".into(), 90)]),
        };

        let result = run_sweep(&conn, &config).unwrap();
        assert_eq!(result.deleted, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn sweep_unknown_type_preserved() {
        let (_dir, conn) = setup_db();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Insert an obs_type not in the config days map
        insert_obs(&conn, "custom_type", now - 9999 * 86400);

        let config = RetentionConfig {
            enabled: true,
            days: HashMap::from([("file_read".into(), 90)]),
        };

        let result = run_sweep(&conn, &config).unwrap();
        assert_eq!(result.deleted, 0);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE obs_type = 'custom_type'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
