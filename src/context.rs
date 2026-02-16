use crate::db::register_udfs;
use crate::summarize::SessionSummary;
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

const INTENTS_SQL: &str = "
SELECT p.id, p.timestamp, p.content,
       COUNT(o.id) AS action_count
FROM prompts p
LEFT JOIN observations o ON o.prompt_id = p.id
JOIN sessions s ON p.session_id = s.id
WHERE p.source = 'user'
  AND s.project = ?1
GROUP BY p.id
HAVING COUNT(o.id) > 0
ORDER BY p.timestamp DESC
LIMIT ?2";

struct IntentRow {
    timestamp: i64,
    content: String,
    action_count: i64,
}

fn query_intents(conn: &Connection, project: &str, limit: i64) -> Result<Vec<IntentRow>, NmemError> {
    let mut stmt = conn.prepare(INTENTS_SQL)?;
    let rows = stmt.query_map(params![project, limit], |row| {
        Ok(IntentRow {
            timestamp: row.get(1)?,
            content: row.get(2)?,
            action_count: row.get(3)?,
        })
    })?
    .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn format_intents(rows: &[IntentRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Recent Intents\n");
    for row in rows {
        let time = format_relative_time(row.timestamp);
        let truncated: String = row.content.chars().take(60).collect();
        let display = if row.content.chars().count() > 60 {
            format!("{truncated}...")
        } else {
            truncated
        };
        let noun = if row.action_count == 1 { "action" } else { "actions" };
        out.push_str(&format!(
            "- [{time}] \"{display}\" → {} {noun}\n",
            row.action_count
        ));
    }
    out
}

const PROJECT_LOCAL_SQL: &str = "
SELECT o.id, o.timestamp, o.obs_type, o.file_path, o.content, o.is_pinned,
       NULL AS project
FROM observations o
JOIN sessions s ON o.session_id = s.id
WHERE s.project = ?1
  AND (
    o.is_pinned = 1
    OR (o.obs_type = 'file_edit' AND o.timestamp > unixepoch('now') - 7200)
    OR (o.obs_type IN ('git_commit', 'git_push') AND o.timestamp > unixepoch('now') - 86400)
  )
ORDER BY o.is_pinned DESC, o.timestamp DESC
LIMIT ?2";

const CROSS_PROJECT_SQL: &str = "
SELECT o.id, o.timestamp, o.obs_type, o.file_path, o.content, o.is_pinned,
       s.project
FROM observations o
JOIN sessions s ON o.session_id = s.id
WHERE s.project IS NOT NULL AND s.project != ?1
  AND o.is_pinned = 1
ORDER BY o.timestamp DESC
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

struct SummaryRow {
    started_at: i64,
    summary: SessionSummary,
}

fn query_summaries(conn: &Connection, project: &str, limit: i64) -> Result<Vec<SummaryRow>, NmemError> {
    let mut stmt = conn.prepare(
        "SELECT started_at, summary FROM sessions
         WHERE project = ?1 AND summary IS NOT NULL
         ORDER BY started_at DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![project, limit], |row| {
            let started_at: i64 = row.get(0)?;
            let summary_str: String = row.get(1)?;
            Ok((started_at, summary_str))
        })?
        .filter_map(|r| {
            let (started_at, summary_str) = r.ok()?;
            let summary: SessionSummary = serde_json::from_str(&summary_str).ok()?;
            Some(SummaryRow { started_at, summary })
        })
        .collect();
    Ok(rows)
}

fn format_summaries(rows: &[SummaryRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Session Summaries\n");
    for (i, row) in rows.iter().enumerate() {
        let time = format_relative_time(row.started_at);
        let intent = &row.summary.intent;
        if intent.is_empty() {
            continue;
        }
        let completed: String = row
            .summary
            .completed
            .iter()
            .take(3)
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if completed.is_empty() {
            out.push_str(&format!("- [{time}] **{intent}**\n"));
        } else {
            out.push_str(&format!("- [{time}] **{intent}** — completed: {completed}\n"));
        }

        // Show learned + next_steps for recent summaries (high-value context)
        if i < 3 && !row.summary.learned.is_empty() {
            let learned = row
                .summary
                .learned
                .iter()
                .take(3)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!("  - Learned: {learned}\n"));
        }
        if i == 0 && !row.summary.next_steps.is_empty() {
            let next = row
                .summary
                .next_steps
                .iter()
                .take(3)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!("  - Next: {next}\n"));
        }
    }
    out
}

/// Generate context injection markdown for a SessionStart event.
/// Returns empty string if no observations exist.
pub fn generate_context(conn: &Connection, project: &str, local_limit: i64, cross_limit: i64) -> Result<String, NmemError> {
    register_udfs(conn)?;

    let intent_rows = query_intents(conn, project, 10)?;
    let summary_rows = query_summaries(conn, project, 10)?;
    let local_rows = query_rows(conn, PROJECT_LOCAL_SQL, project, local_limit)?;
    let cross_rows = query_rows(conn, CROSS_PROJECT_SQL, project, cross_limit)?;

    if intent_rows.is_empty() && summary_rows.is_empty() && local_rows.is_empty() && cross_rows.is_empty() {
        return Ok(String::new());
    }

    let mut out = String::from("# nmem context\n\n");

    let intents = format_intents(&intent_rows);
    if !intents.is_empty() {
        out.push_str(&intents);
        out.push('\n');
    }

    let summaries = format_summaries(&summary_rows);
    if !summaries.is_empty() {
        out.push_str(&summaries);
        out.push('\n');
    }

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
    fn format_intents_empty() {
        assert_eq!(format_intents(&[]), "");
    }

    #[test]
    fn format_intents_basic() {
        let rows = vec![
            IntentRow {
                timestamp: mock_ts(2),
                content: "commit this".into(),
                action_count: 4,
            },
            IntentRow {
                timestamp: mock_ts(5),
                content: "a".repeat(80),
                action_count: 2,
            },
        ];
        let result = format_intents(&rows);
        assert!(result.contains("## Recent Intents"));
        assert!(result.contains("\"commit this\""));
        assert!(result.contains("4 actions"));
        // Truncated content ends with "..."
        assert!(result.contains("...\""));
        assert!(result.contains("2 actions"));
    }

    #[test]
    fn format_intents_singular_action() {
        let rows = vec![IntentRow {
            timestamp: mock_ts(1),
            content: "push it".into(),
            action_count: 1,
        }];
        let result = format_intents(&rows);
        assert!(result.contains("1 action\n"), "should use singular 'action', got: {result}");
        assert!(!result.contains("1 actions"), "should not use plural for count=1");
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
