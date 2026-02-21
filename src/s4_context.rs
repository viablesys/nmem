use crate::db::register_udfs;
use crate::s1_4_summarize::SessionSummary;
use crate::NmemError;
use rusqlite::{Connection, params};

// --- Utility ---

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
        let days_since_epoch = ts / 86400;
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

/// Returns true if the string looks like a URL or is too short to be a useful intent.
fn is_low_quality_intent(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.len() < 10
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
}

// --- Episodes ---

struct EpisodeRow {
    started_at: i64,
    intent: String,
    obs_count: i64,
    hot_files: Vec<String>,
    phase_signature: PhaseInfo,
    summary: Option<String>,
    /// Fallback intent from session summary (used when raw intent is a URL or too short)
    session_intent: Option<String>,
}

struct PhaseInfo {
    investigate: i64,
    execute: i64,
    failures: i64,
    diverge: i64,
    converge: i64,
}

fn query_episodes(conn: &Connection, project: &str, window_secs: i64, limit: i64, before: Option<i64>) -> Result<Vec<EpisodeRow>, NmemError> {
    let now = before.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    });
    let cutoff = now - window_secs;

    let mut stmt = conn.prepare(
        "SELECT w.started_at, w.intent, w.obs_count, w.hot_files, w.phase_signature, w.summary,
                ss.summary AS session_summary
         FROM work_units w
         JOIN sessions ss ON w.session_id = ss.id
         WHERE ss.project = ?1
           AND w.started_at >= ?2
           AND (?4 IS NULL OR w.started_at < ?4)
           AND w.obs_count > 0
         ORDER BY w.started_at DESC
         LIMIT ?3",
    )?;

    let rows = stmt
        .query_map(params![project, cutoff, limit, before], |row| {
            let started_at: i64 = row.get(0)?;
            let intent: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
            let obs_count: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let hot_files_json: String = row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "[]".into());
            let phase_json: String = row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "{}".into());
            let summary: Option<String> = row.get(5)?;
            let session_summary_json: Option<String> = row.get(6)?;
            Ok((started_at, intent, obs_count, hot_files_json, phase_json, summary, session_summary_json))
        })?
        .filter_map(|r| {
            let (started_at, intent, obs_count, hot_files_json, phase_json, summary, session_summary_json) = r.ok()?;
            let hot_files: Vec<String> = serde_json::from_str(&hot_files_json).unwrap_or_default();
            let phase_val: serde_json::Value = serde_json::from_str(&phase_json).unwrap_or_default();
            let phase_signature = PhaseInfo {
                investigate: phase_val.get("investigate").and_then(|v| v.as_i64()).unwrap_or(0),
                execute: phase_val.get("execute").and_then(|v| v.as_i64()).unwrap_or(0),
                failures: phase_val.get("failures").and_then(|v| v.as_i64()).unwrap_or(0),
                diverge: phase_val.get("diverge").and_then(|v| v.as_i64()).unwrap_or(0),
                converge: phase_val.get("converge").and_then(|v| v.as_i64()).unwrap_or(0),
            };
            let session_intent = session_summary_json.and_then(|json| {
                serde_json::from_str::<SessionSummary>(&json).ok().map(|s| s.intent)
            });
            Some(EpisodeRow {
                started_at,
                intent,
                obs_count,
                hot_files,
                phase_signature,
                summary,
                session_intent,
            })
        })
        .collect();

    Ok(rows)
}

fn phase_label(phase: &PhaseInfo) -> String {
    let base = if phase.investigate > phase.execute {
        "investigate"
    } else if phase.execute > phase.investigate {
        "execute"
    } else {
        "mixed"
    };

    let scope = if phase.diverge > 0 || phase.converge > 0 {
        if phase.diverge > phase.converge {
            Some("diverge")
        } else if phase.converge > phase.diverge {
            Some("converge")
        } else {
            None
        }
    } else {
        None
    };

    let mut label = base.to_string();
    if let Some(s) = scope {
        label = format!("{label}→{s}");
    }
    if phase.failures > 0 {
        label = format!("{label}+failures");
    }
    label
}

fn format_episodes(rows: &[EpisodeRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Recent Episodes\n");
    for (i, row) in rows.iter().enumerate() {
        let time = format_relative_time(row.started_at);

        // Use session summary intent as fallback when raw intent is low quality
        let raw_intent = &row.intent;
        let intent_source = if is_low_quality_intent(raw_intent) {
            row.session_intent.as_deref().unwrap_or(raw_intent)
        } else {
            raw_intent
        };
        let intent_display: String = intent_source.chars().take(80).collect();
        let intent_display = if intent_source.chars().count() > 80 {
            format!("{intent_display}...")
        } else {
            intent_display
        };

        let phase = phase_label(&row.phase_signature);
        out.push_str(&format!(
            "- [{time}] **{intent_display}** ({} obs, {phase})\n",
            row.obs_count
        ));

        // Show files and learned for the most recent 3 episodes
        if i < 3 && !row.hot_files.is_empty() {
            let files: String = row.hot_files.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
            out.push_str(&format!("  - Files: {files}\n"));
        }

        if i < 3
            && let Some(summary) = &row.summary
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(summary)
                    && let Some(learned) = val.get("learned") {
                        let learned_items = match learned {
                            serde_json::Value::Array(arr) => arr
                                .iter()
                                .take(3)
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join("; "),
                            serde_json::Value::String(s) => s.clone(),
                            _ => String::new(),
                        };
                        if !learned_items.is_empty() {
                            out.push_str(&format!("  - Learned: {learned_items}\n"));
                        }
                    }
    }
    out
}

// --- Fallback summaries (outside episode window) ---

struct SummaryRow {
    started_at: i64,
    summary: SessionSummary,
}

fn query_fallback_summaries(conn: &Connection, project: &str, window_secs: i64, limit: i64, before: Option<i64>) -> Result<Vec<SummaryRow>, NmemError> {
    let now = before.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    });
    let cutoff = now - window_secs;

    // Sessions older than the episode window, OR sessions without episodes
    let mut stmt = conn.prepare(
        "SELECT s.started_at, s.summary FROM sessions s
         WHERE s.project = ?1 AND s.summary IS NOT NULL
           AND (?4 IS NULL OR s.started_at < ?4)
           AND (s.started_at < ?2
                OR NOT EXISTS (SELECT 1 FROM work_units w WHERE w.session_id = s.id))
         ORDER BY s.started_at DESC LIMIT ?3",
    )?;

    let rows = stmt
        .query_map(params![project, cutoff, limit, before], |row| {
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
        out.push_str(&format!("- [{time}] **{intent}**\n"));

        // Show learned for the first 3 summaries only (high-value context)
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
    }
    out
}

// --- Suggested tasks ---

fn query_suggested_tasks(conn: &Connection, project: &str, limit: i64) -> Result<Vec<String>, NmemError> {
    let mut tasks = Vec::new();

    // Gather next_steps from the most recent session summary
    let mut stmt = conn.prepare(
        "SELECT summary FROM sessions
         WHERE project = ?1 AND summary IS NOT NULL
         ORDER BY started_at DESC LIMIT 1",
    )?;
    let summary_rows: Vec<String> = stmt
        .query_map(params![project], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    for summary_str in &summary_rows {
        if let Ok(summary) = serde_json::from_str::<SessionSummary>(summary_str) {
            for step in summary.next_steps.iter().take(limit as usize) {
                tasks.push(step.clone());
            }
        }
    }

    // Also gather from recent episode narratives that have next_steps
    let mut ep_stmt = conn.prepare(
        "SELECT w.summary FROM work_units w
         JOIN sessions s ON w.session_id = s.id
         WHERE s.project = ?1 AND w.summary IS NOT NULL
         ORDER BY w.started_at DESC LIMIT 5",
    )?;
    let ep_summaries: Vec<String> = ep_stmt
        .query_map(params![project], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    for ep_summary in &ep_summaries {
        if tasks.len() >= limit as usize {
            break;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(ep_summary)
            && let Some(serde_json::Value::Array(steps)) = val.get("next_steps") {
                for step in steps.iter().filter_map(|v| v.as_str()) {
                    if tasks.len() >= limit as usize {
                        break;
                    }
                    if !tasks.iter().any(|t| t == step) {
                        tasks.push(step.to_string());
                    }
                }
            }
    }

    Ok(tasks)
}

fn format_suggested_tasks(tasks: &[String]) -> String {
    if tasks.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Suggested Tasks\n");
    for task in tasks {
        out.push_str(&format!("- {task}\n"));
    }
    out
}

// --- Compressed observation table ---

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
SELECT o.id, o.timestamp, o.obs_type, o.file_path, o.content, o.is_pinned,
       NULL AS project
FROM observations o
JOIN sessions s ON o.session_id = s.id
WHERE s.project = ?1
  AND (?3 IS NULL OR o.timestamp < ?3)
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
  AND (?3 IS NULL OR o.timestamp < ?3)
ORDER BY o.timestamp DESC
LIMIT ?2";

fn query_rows(conn: &Connection, sql: &str, project: &str, limit: i64, before: Option<i64>) -> Result<Vec<ContextRow>, NmemError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project, limit, before], |row| {
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

/// Compress observation rows into grouped activity lines.
/// Pinned items and git operations stay as individual rows.
/// File edits are grouped by path with counts.
fn format_activity(rows: &[ContextRow], header: &str) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(header);
    out.push('\n');

    // Separate into individual items (pinned, git ops) and grouped edits
    let mut individual: Vec<&ContextRow> = Vec::new();
    let mut edit_groups: std::collections::BTreeMap<String, (i64, i64, String)> = std::collections::BTreeMap::new(); // path -> (count, most_recent_ts, obs_type)

    for row in rows {
        if row.is_pinned || row.obs_type == "git_commit" || row.obs_type == "git_push" {
            individual.push(row);
        } else if let Some(fp) = &row.file_path {
            let entry = edit_groups.entry(fp.clone()).or_insert((0, row.timestamp, row.obs_type.clone()));
            entry.0 += 1;
            if row.timestamp > entry.1 {
                entry.1 = row.timestamp;
            }
        } else {
            individual.push(row);
        }
    }

    // Sort grouped edits by most recent timestamp descending
    let mut grouped: Vec<(String, i64, i64, String)> = edit_groups
        .into_iter()
        .map(|(path, (count, ts, obs_type))| (path, count, ts, obs_type))
        .collect();
    grouped.sort_by(|a, b| b.2.cmp(&a.2));

    // Format grouped edits
    for (path, count, ts, _obs_type) in &grouped {
        let time = format_relative_time(*ts);
        let path = path.replace('|', "\\|");
        if *count == 1 {
            out.push_str(&format!("- {path} ({time})\n"));
        } else {
            out.push_str(&format!("- {path} — {count} edits ({time})\n"));
        }
    }

    // Format individual items (pinned, git ops)
    for row in &individual {
        let time = format_relative_time(row.timestamp);
        let title = title_for_row(row);
        let title = title.replace('|', "\\|");
        let project_suffix = if let Some(p) = &row.project {
            format!(" [{p}]")
        } else {
            String::new()
        };
        let pin = if row.is_pinned { " (pinned)" } else { "" };
        out.push_str(&format!(
            "- #{} {} {}{}{}{}\n",
            row.id, row.obs_type, title, project_suffix, pin,
            if !pin.is_empty() { String::new() } else { format!(" ({time})") }
        ));
    }

    out
}

// --- Main generation ---

/// Generate context injection markdown for a SessionStart event.
/// Returns empty string if no observations exist.
pub fn generate_context(conn: &Connection, project: &str, local_limit: i64, cross_limit: i64, before: Option<i64>) -> Result<String, NmemError> {
    register_udfs(conn)?;

    let config = crate::config::load_config().unwrap_or_default();
    let episode_window = crate::config::resolve_episode_window(&config, project);

    let episode_rows = query_episodes(conn, project, episode_window, 15, before)?;
    let summary_rows = query_fallback_summaries(conn, project, episode_window, 5, before)?;
    let suggested = query_suggested_tasks(conn, project, 5)?;
    let local_rows = query_rows(conn, PROJECT_LOCAL_SQL, project, local_limit, before)?;
    let cross_rows = query_rows(conn, CROSS_PROJECT_SQL, project, cross_limit, before)?;

    if episode_rows.is_empty() && summary_rows.is_empty()
        && local_rows.is_empty() && cross_rows.is_empty()
    {
        return Ok(String::new());
    }

    let mut out = String::from("# nmem context\n\n");

    let episodes = format_episodes(&episode_rows);
    if !episodes.is_empty() {
        out.push_str(&episodes);
        out.push('\n');
    }

    let summaries = format_summaries(&summary_rows);
    if !summaries.is_empty() {
        out.push_str(&summaries);
        out.push('\n');
    }

    let tasks = format_suggested_tasks(&suggested);
    if !tasks.is_empty() {
        out.push_str(&tasks);
        out.push('\n');
    }

    let activity = format_activity(&local_rows, &format!("## {project}"));
    if !activity.is_empty() {
        out.push_str(&activity);
    }

    if !cross_rows.is_empty() {
        out.push('\n');
        out.push_str(&format_activity(&cross_rows, "## Other projects"));
    }

    Ok(out)
}

/// CLI handler: print context injection output for the current project.
pub fn handle_context(db_path: &std::path::Path, args: &crate::cli::ContextArgs) -> Result<(), NmemError> {
    let conn = crate::db::open_db_readonly(db_path)?;

    let project = args.project.clone().unwrap_or_else(|| {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        crate::project::derive_project(&cwd)
    });

    let config = crate::config::load_config()?;
    let (local_limit, cross_limit) = crate::config::resolve_context_limits(&config, &project, false);

    let ctx = generate_context(&conn, &project, local_limit, cross_limit, None)?;
    if ctx.is_empty() {
        println!("No context available for project \"{project}\".");
    } else {
        print!("{ctx}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MIGRATIONS;

    fn mock_ts(minutes_ago: i64) -> i64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now - (minutes_ago * 60)
    }

    fn setup_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn now_ts() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
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
        let ts = mock_ts(10 * 24 * 60);
        let result = format_relative_time(ts);
        assert!(result.len() >= 5, "expected date format, got: {result}");
        assert!(!result.contains("ago"), "should not use relative format for >7 days");
    }

    #[test]
    fn is_low_quality_intent_detects_urls() {
        assert!(is_low_quality_intent("https://github.com/foo/bar"));
        assert!(is_low_quality_intent("http://localhost:1234"));
        assert!(!is_low_quality_intent("fix the authentication bug"));
    }

    #[test]
    fn is_low_quality_intent_detects_short() {
        assert!(is_low_quality_intent("yes"));
        assert!(is_low_quality_intent("ok do it"));
        assert!(!is_low_quality_intent("implement the episode detection system"));
    }

    #[test]
    fn format_episodes_empty() {
        assert_eq!(format_episodes(&[]), "");
    }

    #[test]
    fn format_episodes_basic() {
        let rows = vec![EpisodeRow {
            started_at: mock_ts(10),
            intent: "fix the authentication bug in the login handler".into(),
            obs_count: 5,
            hot_files: vec!["src/auth.rs".into(), "src/handler.rs".into()],
            phase_signature: PhaseInfo { investigate: 2, execute: 3, failures: 0, diverge: 0, converge: 0 },
            summary: None,
            session_intent: None,
        }];
        let result = format_episodes(&rows);
        assert!(result.contains("## Recent Episodes"));
        assert!(result.contains("**fix the authentication bug"));
        assert!(result.contains("5 obs"));
        assert!(result.contains("execute"));
        assert!(result.contains("src/auth.rs"));
    }

    #[test]
    fn format_episodes_url_intent_falls_back() {
        let rows = vec![EpisodeRow {
            started_at: mock_ts(5),
            intent: "https://github.com/foo/bar/blob/main/doc.md".into(),
            obs_count: 10,
            hot_files: vec![],
            phase_signature: PhaseInfo { investigate: 5, execute: 5, failures: 0, diverge: 0, converge: 0 },
            summary: None,
            session_intent: Some("Implement Bayesian surprise in episodic memory".into()),
        }];
        let result = format_episodes(&rows);
        assert!(result.contains("Implement Bayesian surprise"), "should use session intent fallback");
        assert!(!result.contains("github.com"), "should not show URL as intent");
    }

    #[test]
    fn format_episodes_short_intent_falls_back() {
        let rows = vec![EpisodeRow {
            started_at: mock_ts(5),
            intent: "yes".into(),
            obs_count: 8,
            hot_files: vec![],
            phase_signature: PhaseInfo { investigate: 0, execute: 8, failures: 0, diverge: 0, converge: 0 },
            summary: None,
            session_intent: Some("Refactor dispatch queue logic".into()),
        }];
        let result = format_episodes(&rows);
        assert!(result.contains("Refactor dispatch"), "should use session intent for short prompts");
    }

    #[test]
    fn format_episodes_with_failures() {
        let rows = vec![EpisodeRow {
            started_at: mock_ts(5),
            intent: "debug the test".into(),
            obs_count: 3,
            hot_files: vec![],
            phase_signature: PhaseInfo { investigate: 3, execute: 1, failures: 2, diverge: 0, converge: 0 },
            summary: None,
            session_intent: None,
        }];
        let result = format_episodes(&rows);
        assert!(result.contains("investigate+failures"));
    }

    #[test]
    fn format_episodes_with_learned() {
        let rows = vec![EpisodeRow {
            started_at: mock_ts(5),
            intent: "fix auth".into(),
            obs_count: 4,
            hot_files: vec![],
            phase_signature: PhaseInfo { investigate: 1, execute: 1, failures: 0, diverge: 0, converge: 0 },
            summary: Some(r#"{"learned":["stale mocks cause failures","update mock first"]}"#.into()),
            session_intent: None,
        }];
        let result = format_episodes(&rows);
        assert!(result.contains("Learned: stale mocks cause failures; update mock first"));
    }

    #[test]
    fn phase_label_variants() {
        assert_eq!(phase_label(&PhaseInfo { investigate: 5, execute: 2, failures: 0, diverge: 0, converge: 0 }), "investigate");
        assert_eq!(phase_label(&PhaseInfo { investigate: 2, execute: 5, failures: 0, diverge: 0, converge: 0 }), "execute");
        assert_eq!(phase_label(&PhaseInfo { investigate: 3, execute: 3, failures: 0, diverge: 0, converge: 0 }), "mixed");
        assert_eq!(phase_label(&PhaseInfo { investigate: 5, execute: 2, failures: 1, diverge: 0, converge: 0 }), "investigate+failures");
        assert_eq!(phase_label(&PhaseInfo { investigate: 5, execute: 2, failures: 0, diverge: 3, converge: 1 }), "investigate→diverge");
        assert_eq!(phase_label(&PhaseInfo { investigate: 2, execute: 5, failures: 0, diverge: 1, converge: 4 }), "execute→converge");
        assert_eq!(phase_label(&PhaseInfo { investigate: 5, execute: 2, failures: 1, diverge: 2, converge: 5 }), "investigate→converge+failures");
    }

    #[test]
    fn format_suggested_tasks_empty() {
        assert_eq!(format_suggested_tasks(&[]), "");
    }

    #[test]
    fn format_suggested_tasks_basic() {
        let tasks = vec!["Run cargo test".into(), "Update docs".into()];
        let result = format_suggested_tasks(&tasks);
        assert!(result.contains("## Suggested Tasks"));
        assert!(result.contains("- Run cargo test"));
        assert!(result.contains("- Update docs"));
    }

    #[test]
    fn format_activity_empty() {
        assert_eq!(format_activity(&[], "## Test"), "");
    }

    #[test]
    fn format_activity_groups_edits() {
        let rows = vec![
            ContextRow {
                id: 1, timestamp: mock_ts(1), obs_type: "file_edit".into(),
                file_path: Some("src/main.rs".into()), content: String::new(),
                is_pinned: false, project: None,
            },
            ContextRow {
                id: 2, timestamp: mock_ts(2), obs_type: "file_edit".into(),
                file_path: Some("src/main.rs".into()), content: String::new(),
                is_pinned: false, project: None,
            },
            ContextRow {
                id: 3, timestamp: mock_ts(3), obs_type: "file_edit".into(),
                file_path: Some("src/main.rs".into()), content: String::new(),
                is_pinned: false, project: None,
            },
        ];
        let result = format_activity(&rows, "## myproj");
        assert!(result.contains("## myproj"));
        assert!(result.contains("src/main.rs — 3 edits"), "should group edits: {result}");
        assert!(!result.contains("#1"), "should not show individual IDs for grouped edits");
    }

    #[test]
    fn format_activity_pinned_stays_individual() {
        let rows = vec![
            ContextRow {
                id: 42, timestamp: mock_ts(5), obs_type: "command".into(),
                file_path: None, content: "important-cmd".into(),
                is_pinned: true, project: None,
            },
        ];
        let result = format_activity(&rows, "## myproj");
        assert!(result.contains("#42"), "pinned should show ID");
        assert!(result.contains("(pinned)"), "pinned should show marker");
        assert!(result.contains("important-cmd"), "pinned should show content");
    }

    #[test]
    fn format_activity_git_ops_individual() {
        let rows = vec![
            ContextRow {
                id: 100, timestamp: mock_ts(10), obs_type: "git_commit".into(),
                file_path: None, content: "git commit -m 'fix auth'".into(),
                is_pinned: false, project: None,
            },
        ];
        let result = format_activity(&rows, "## myproj");
        assert!(result.contains("#100"), "git ops should show ID");
        assert!(result.contains("git_commit"), "git ops should show type");
    }

    #[test]
    fn format_activity_single_edit_no_count() {
        let rows = vec![
            ContextRow {
                id: 1, timestamp: mock_ts(1), obs_type: "file_edit".into(),
                file_path: Some("src/lib.rs".into()), content: String::new(),
                is_pinned: false, project: None,
            },
        ];
        let result = format_activity(&rows, "## myproj");
        assert!(result.contains("src/lib.rs ("), "single edit should show path with time");
        assert!(!result.contains("edits"), "single edit should not say 'edits'");
    }

    #[test]
    fn query_episodes_filters_zero_obs() {
        let conn = setup_db();
        let ts = now_ts();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', ?1)",
            [ts - 3600],
        ).unwrap();
        // Episode with 0 obs — should be filtered out
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count, hot_files, phase_signature)
             VALUES ('s1', ?1, 'just a question', 0, '[]', '{}')",
            [ts - 3600],
        ).unwrap();
        // Episode with real obs — should appear
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count, hot_files, phase_signature)
             VALUES ('s1', ?1, 'fix auth bug', 5, '[\"src/auth.rs\"]', '{\"investigate\":2,\"execute\":3,\"failures\":0}')",
            [ts - 3000],
        ).unwrap();

        register_udfs(&conn).unwrap();
        let rows = query_episodes(&conn, "test", 48 * 3600, 15, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].intent, "fix auth bug");
    }

    #[test]
    fn query_episodes_within_window() {
        let conn = setup_db();
        let ts = now_ts();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', ?1)",
            [ts - 3600],
        ).unwrap();
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count, hot_files, phase_signature)
             VALUES ('s1', ?1, 'fix auth bug', 5, '[\"src/auth.rs\"]', '{\"investigate\":2,\"execute\":3,\"failures\":0}')",
            [ts - 3600],
        ).unwrap();

        register_udfs(&conn).unwrap();
        let rows = query_episodes(&conn, "test", 48 * 3600, 15, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].intent, "fix auth bug");
        assert_eq!(rows[0].obs_count, 5);
        assert_eq!(rows[0].hot_files, vec!["src/auth.rs"]);
    }

    #[test]
    fn query_episodes_outside_window() {
        let conn = setup_db();
        let ts = now_ts();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', ?1)",
            [ts - 200000],
        ).unwrap();
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count, hot_files, phase_signature)
             VALUES ('s1', ?1, 'old episode', 3, '[]', '{}')",
            [ts - 200000],
        ).unwrap();

        register_udfs(&conn).unwrap();
        let rows = query_episodes(&conn, "test", 3600, 15, None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn query_fallback_summaries_excludes_episoded_sessions() {
        let conn = setup_db();
        let ts = now_ts();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at, summary) VALUES ('s1', 'test', ?1, ?2)",
            params![ts - 3600, r#"{"intent":"recent with episodes","completed":[],"learned":[],"next_steps":[],"files_read":[],"files_edited":[],"notes":null}"#],
        ).unwrap();
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count) VALUES ('s1', ?1, 'ep1', 5)",
            [ts - 3600],
        ).unwrap();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at, summary) VALUES ('s2', 'test', ?1, ?2)",
            params![ts - 7200, r#"{"intent":"recent no episodes","completed":[],"learned":[],"next_steps":[],"files_read":[],"files_edited":[],"notes":null}"#],
        ).unwrap();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at, summary) VALUES ('s3', 'test', ?1, ?2)",
            params![ts - 200000, r#"{"intent":"old session","completed":[],"learned":[],"next_steps":[],"files_read":[],"files_edited":[],"notes":null}"#],
        ).unwrap();

        let rows = query_fallback_summaries(&conn, "test", 48 * 3600, 10, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].summary.intent, "recent no episodes");
        assert_eq!(rows[1].summary.intent, "old session");
    }

    #[test]
    fn query_suggested_tasks_from_session() {
        let conn = setup_db();
        let ts = now_ts();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at, summary) VALUES ('s1', 'test', ?1, ?2)",
            params![ts - 3600, r#"{"intent":"work","completed":[],"learned":[],"next_steps":["Run cargo test","Update docs"],"files_read":[],"files_edited":[],"notes":null}"#],
        ).unwrap();

        let tasks = query_suggested_tasks(&conn, "test", 5).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0], "Run cargo test");
        assert_eq!(tasks[1], "Update docs");
    }

    #[test]
    fn generate_context_with_episodes() {
        let conn = setup_db();
        register_udfs(&conn).unwrap();
        let ts = now_ts();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', ?1)",
            [ts - 3600],
        ).unwrap();

        conn.execute(
            "INSERT INTO prompts (session_id, timestamp, source, content) VALUES ('s1', ?1, 'user', 'fix the bug')",
            [ts - 3600],
        ).unwrap();
        let prompt_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content, file_path)
             VALUES ('s1', ?1, ?2, 'file_edit', 'PostToolUse', 'edited', '/src/auth.rs')",
            params![prompt_id, ts - 3500],
        ).unwrap();

        conn.execute(
            "INSERT INTO work_units (session_id, started_at, intent, obs_count, hot_files, phase_signature)
             VALUES ('s1', ?1, 'fix auth bug', 5, '[\"src/auth.rs\"]', '{\"investigate\":2,\"execute\":3,\"failures\":0}')",
            [ts - 3600],
        ).unwrap();

        let ctx = generate_context(&conn, "test", 20, 10, None).unwrap();
        assert!(ctx.contains("# nmem context"));
        assert!(ctx.contains("## Recent Episodes"));
        assert!(ctx.contains("fix auth bug"));
        assert!(ctx.contains("src/auth.rs"));
        // Intents section should NOT be present
        assert!(!ctx.contains("## Recent Intents"), "intents section should be removed");
    }
}
