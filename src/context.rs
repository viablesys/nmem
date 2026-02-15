use crate::serve::register_udfs;
use crate::NmemError;
use rusqlite::{Connection, params};

struct ContextRow {
    id: i64,
    timestamp: i64,
    obs_type: String,
    file_path: Option<String>,
    content: String,
    is_pinned: bool,
    project: Option<String>,
}

const PROJECT_LOCAL_SQL: &str = "
WITH scored AS (
    SELECT o.id, o.timestamp, o.obs_type, o.file_path, o.content, o.is_pinned,
           NULL AS project,
           exp_decay((unixepoch('now') - o.timestamp) / 86400.0, 7.0) AS recency,
           CASE o.obs_type
               WHEN 'file_edit' THEN 1.0 WHEN 'command' THEN 0.67
               WHEN 'session_compact' THEN 0.5 WHEN 'mcp_call' THEN 0.33
               ELSE 0.17
           END AS type_w
    FROM observations o
    JOIN sessions s ON o.session_id = s.id
    WHERE s.project = ?1
),
ranked AS (
    SELECT *,
           (recency * 0.6 + type_w * 0.4) AS score,
           ROW_NUMBER() OVER (
               PARTITION BY COALESCE(file_path, CAST(id AS TEXT))
               ORDER BY (recency * 0.6 + type_w * 0.4) DESC
           ) AS rn
    FROM scored
)
SELECT id, timestamp, obs_type, file_path, content, is_pinned, project, score
FROM ranked WHERE rn = 1
ORDER BY score DESC
LIMIT ?2";

const CROSS_PROJECT_SQL: &str = "
WITH scored AS (
    SELECT o.id, o.timestamp, o.obs_type, o.file_path, o.content, o.is_pinned,
           s.project,
           exp_decay((unixepoch('now') - o.timestamp) / 86400.0, 7.0) AS recency,
           CASE o.obs_type
               WHEN 'file_edit' THEN 1.0 WHEN 'command' THEN 0.67
               WHEN 'session_compact' THEN 0.5 WHEN 'mcp_call' THEN 0.33
               ELSE 0.17
           END AS type_w
    FROM observations o
    JOIN sessions s ON o.session_id = s.id
    WHERE s.project IS NOT NULL AND s.project != ?1
),
ranked AS (
    SELECT *,
           (recency * 0.6 + type_w * 0.4) AS score,
           ROW_NUMBER() OVER (
               PARTITION BY COALESCE(file_path, CAST(id AS TEXT))
               ORDER BY (recency * 0.6 + type_w * 0.4) DESC
           ) AS rn
    FROM scored
)
SELECT id, timestamp, obs_type, file_path, content, is_pinned, project, score
FROM ranked WHERE rn = 1
ORDER BY score DESC
LIMIT ?2";

fn query_rows(conn: &Connection, sql: &str, project: &str, limit: i64) -> Result<Vec<ContextRow>, NmemError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project, limit], |row| {
        Ok(ContextRow {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            obs_type: row.get(2)?,
            file_path: row.get(3)?,
            content: row.get(4)?,
            is_pinned: row.get::<_, i64>(5)? != 0,
            project: row.get(6)?,
        })
    })?
    .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn format_relative_time(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now - ts;

    if diff < 3600 {
        let mins = (diff / 60).max(1);
        format!("{mins}m ago")
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{hours}h ago")
    } else if diff < 604800 {
        let days = diff / 86400;
        format!("{days}d ago")
    } else {
        // Format as "Mon DD"
        // Simple calculation without chrono dependency
        let secs = ts;
        // Days since epoch
        let days_since_epoch = secs / 86400;
        // Approximate date calculation
        let (year, month, day) = days_to_ymd(days_since_epoch);
        let month_name = match month {
            1 => "Jan", 2 => "Feb", 3 => "Mar", 4 => "Apr",
            5 => "May", 6 => "Jun", 7 => "Jul", 8 => "Aug",
            9 => "Sep", 10 => "Oct", 11 => "Nov", 12 => "Dec",
            _ => "???",
        };
        if year == current_year() {
            format!("{month_name} {day:02}")
        } else {
            format!("{month_name} {day:02}, {year}")
        }
    }
}

fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
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

fn current_year() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    days_to_ymd(now / 86400).0
}

fn title_for_row(row: &ContextRow) -> String {
    if let Some(fp) = &row.file_path {
        fp.clone()
    } else {
        let s: String = row.content.chars().take(60).collect();
        if row.content.len() > 60 {
            format!("{s}...")
        } else {
            s
        }
    }
}

fn format_table(rows: &[ContextRow], header: &str) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(header);
    out.push('\n');
    out.push_str("| ID | Time | Type | Title | Pin |\n");
    out.push_str("|----|------|------|-------|-----|\n");

    for row in rows {
        let pin = if row.is_pinned { "*" } else { "" };
        let time = format_relative_time(row.timestamp);
        let title = title_for_row(row);
        // Escape pipes in title
        let title = title.replace('|', "\\|");
        let project_suffix = if let Some(p) = &row.project {
            format!(" [{p}]")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "| #{} | {} | {} | {}{} | {} |\n",
            row.id, time, row.obs_type, title, project_suffix, pin
        ));
    }

    out
}

/// Generate context injection markdown for a SessionStart event.
/// Returns empty string if no observations exist.
pub fn generate_context(conn: &Connection, project: &str, source: &str) -> Result<String, NmemError> {
    register_udfs(conn)?;

    let is_recovery = matches!(source, "compact" | "clear");
    let local_limit: i64 = if is_recovery { 30 } else { 20 };
    let cross_limit: i64 = if is_recovery { 15 } else { 10 };

    let local_rows = query_rows(conn, PROJECT_LOCAL_SQL, project, local_limit)?;
    let cross_rows = query_rows(conn, CROSS_PROJECT_SQL, project, cross_limit)?;

    if local_rows.is_empty() && cross_rows.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::from("# nmem context\n\n");
    out.push_str(&format_table(&local_rows, &format!("## {project}")));

    if !cross_rows.is_empty() {
        out.push('\n');
        out.push_str(&format_table(&cross_rows, "## Other projects"));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_ts(minutes_ago: i64) -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now - (minutes_ago * 60)
    }

    #[test]
    fn format_relative_time_minutes() {
        let ts = mock_ts(30);
        let result = format_relative_time(ts);
        assert_eq!(result, "30m ago");
    }

    #[test]
    fn format_relative_time_hours() {
        let ts = mock_ts(5 * 60);
        let result = format_relative_time(ts);
        assert_eq!(result, "5h ago");
    }

    #[test]
    fn format_relative_time_days() {
        let ts = mock_ts(3 * 24 * 60);
        let result = format_relative_time(ts);
        assert_eq!(result, "3d ago");
    }

    #[test]
    fn format_relative_time_old() {
        // 10 days ago
        let ts = mock_ts(10 * 24 * 60);
        let result = format_relative_time(ts);
        // Should be "Mon DD" format
        assert!(result.len() >= 5, "expected date format, got: {result}");
        assert!(!result.contains("ago"), "should not use relative format for >7 days");
    }

    #[test]
    fn title_for_row_with_file_path() {
        let row = ContextRow {
            id: 1,
            timestamp: 0,
            obs_type: "file_edit".into(),
            file_path: Some("src/main.rs".into()),
            content: "some content".into(),
            is_pinned: false,
            project: None,
        };
        assert_eq!(title_for_row(&row), "src/main.rs");
    }

    #[test]
    fn title_for_row_truncates_content() {
        let row = ContextRow {
            id: 1,
            timestamp: 0,
            obs_type: "command".into(),
            file_path: None,
            content: "a".repeat(100),
            is_pinned: false,
            project: None,
        };
        let title = title_for_row(&row);
        assert!(title.ends_with("..."));
        assert!(title.len() <= 64); // 60 chars + "..."
    }

    #[test]
    fn format_table_empty() {
        assert_eq!(format_table(&[], "## Test"), "");
    }

    #[test]
    fn format_table_basic() {
        let rows = vec![ContextRow {
            id: 42,
            timestamp: mock_ts(5),
            obs_type: "file_edit".into(),
            file_path: Some("src/lib.rs".into()),
            content: String::new(),
            is_pinned: true,
            project: None,
        }];
        let result = format_table(&rows, "## myproj");
        assert!(result.contains("## myproj"));
        assert!(result.contains("#42"));
        assert!(result.contains("file_edit"));
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("*"));
    }
}
