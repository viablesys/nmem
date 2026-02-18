use crate::cli::LearnArgs;
use crate::db::open_db_readonly;
use crate::NmemError;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Pattern {
    pub kind: &'static str,
    pub description: String,
    pub normalized: String,
    pub session_count: i64,
    pub heat: f64,
    pub sessions: Vec<String>,
    pub example: String,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Exponential decay: recent events score ~1.0, old events approach 0.
fn exp_decay(age_hours: f64, half_life_hours: f64) -> f64 {
    if half_life_hours <= 0.0 {
        return 0.0;
    }
    (-std::f64::consts::LN_2 * age_hours / half_life_hours).exp()
}

/// Commands where non-zero exit is expected behavior, not a real failure.
fn is_diagnostic(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or("");
    // Probe commands
    if matches!(first, "which" | "type" | "command" | "hash") {
        return true;
    }
    // tmux cleanup
    if cmd.starts_with("tmux kill-") || cmd.starts_with("tmux has-session") {
        return true;
    }
    // Sleep-then-check patterns
    if cmd.starts_with("sleep ") {
        return true;
    }
    // Shell sourcing chains (source ... ; cmd or source ... && cmd)
    if cmd.starts_with("source ") || cmd.starts_with(". ") {
        return true;
    }
    // Compound probes: "which X && Y", "export ... && Y"
    if cmd.starts_with("export ") {
        return true;
    }
    false
}

/// Strip noise from command strings for grouping.
/// Removes trailing redirects, path prefixes, pipe tails, subcommand args.
fn normalize_command(raw: &str) -> String {
    let mut s = raw.to_string();

    // /home/*/ — strip user dir first (so subsequent prefix checks see relative paths)
    if let Some(rest) = s.strip_prefix("/home/")
        && let Some(idx) = rest.find('/')
    {
        s = rest[idx + 1..].to_string();
    }

    // Strip common path prefixes
    for prefix in ["~/.cargo/bin/", ".cargo/bin/", "/usr/local/bin/", "/usr/bin/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }

    // Strip trailing pipe to tail/head (before 2>&1 so we catch "cmd 2>&1 | tail -5")
    if let Some(idx) = s.rfind('|') {
        let tail = s[idx + 1..].trim();
        if tail.starts_with("tail") || tail.starts_with("head") {
            s = s[..idx].trim_end().to_string();
        }
    }

    // Strip trailing 2>&1
    let trimmed = s.trim_end();
    if let Some(stripped) = trimmed.strip_suffix("2>&1") {
        s = stripped.trim_end().to_string();
    } else {
        s = trimmed.to_string();
    }

    // Collapse subcommand args: "cargo test foo" → "cargo test"
    // Keep first two tokens for commands with subcommands.
    let s = s.trim();
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let first = tokens.first().copied().unwrap_or("");
    if matches!(first, "cargo" | "npm" | "git" | "docker" | "kubectl" | "go") && tokens.len() > 2 {
        format!("{} {}", tokens[0], tokens[1])
    } else {
        s.to_string()
    }
}

/// Detect repeated failed commands across sessions.
fn detect_failed_commands(
    conn: &Connection,
    threshold: i64,
    half_life: f64,
) -> Result<Vec<Pattern>, NmemError> {
    let now = now_secs();

    // Fetch per-session failed commands with latest timestamp per (content, session).
    let mut stmt = conn.prepare(
        "SELECT content, session_id, MAX(timestamp) as latest_ts
         FROM observations
         WHERE obs_type = 'command'
           AND json_extract(metadata, '$.failed') = 1
         GROUP BY content, session_id",
    )?;

    struct Row {
        content: String,
        session_id: String,
        timestamp: i64,
    }

    let rows: Vec<Row> = stmt
        .query_map([], |row| {
            Ok(Row {
                content: row.get(0)?,
                session_id: row.get(1)?,
                timestamp: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    // Group by normalized command
    struct Group {
        sessions: HashMap<String, i64>, // session_id → latest timestamp
        example: String,
    }

    let mut groups: HashMap<String, Group> = HashMap::new();
    for row in &rows {
        let norm = normalize_command(&row.content);
        if is_diagnostic(&norm) {
            continue;
        }
        let group = groups.entry(norm).or_insert_with(|| Group {
            sessions: HashMap::new(),
            example: row.content.clone(),
        });
        group
            .sessions
            .entry(row.session_id.clone())
            .and_modify(|ts| *ts = (*ts).max(row.timestamp))
            .or_insert(row.timestamp);
    }

    let mut patterns: Vec<Pattern> = groups
        .into_iter()
        .filter(|(_, g)| g.sessions.len() as i64 >= threshold)
        .map(|(norm, g)| {
            let heat: f64 = g
                .sessions
                .values()
                .map(|ts| {
                    let age_hours = (now - ts) as f64 / 3600.0;
                    exp_decay(age_hours, half_life)
                })
                .sum();
            let session_count = g.sessions.len() as i64;
            let sessions: Vec<String> = g.sessions.into_keys().collect();
            Pattern {
                kind: "failed_command",
                description: format!("`{}` failed across {session_count} sessions", short_cmd(&norm)),
                normalized: norm,
                session_count,
                heat,
                sessions,
                example: g.example,
            }
        })
        .collect();

    patterns.sort_by(|a, b| b.heat.partial_cmp(&a.heat).unwrap_or(std::cmp::Ordering::Equal));
    patterns.truncate(20);
    Ok(patterns)
}

/// Detect files read in multiple sessions but never edited.
fn detect_unresolved_reads(
    conn: &Connection,
    threshold: i64,
    half_life: f64,
) -> Result<Vec<Pattern>, NmemError> {
    let now = now_secs();

    // Per-session reads with latest timestamp, excluding files that were ever edited.
    let mut stmt = conn.prepare(
        "SELECT o.file_path, o.session_id, MAX(o.timestamp) as latest_ts
         FROM observations o
         WHERE o.obs_type = 'file_read'
           AND o.file_path IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM observations e
               WHERE e.file_path = o.file_path
                 AND e.obs_type IN ('file_edit', 'file_write')
           )
         GROUP BY o.file_path, o.session_id",
    )?;

    struct Row {
        file_path: String,
        session_id: String,
        timestamp: i64,
    }

    let rows: Vec<Row> = stmt
        .query_map([], |row| {
            Ok(Row {
                file_path: row.get(0)?,
                session_id: row.get(1)?,
                timestamp: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    // Group by file_path, excluding reference-only paths
    let mut groups: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for row in &rows {
        if is_reference_path(&row.file_path) {
            continue;
        }
        groups
            .entry(row.file_path.clone())
            .or_default()
            .entry(row.session_id.clone())
            .and_modify(|ts| *ts = (*ts).max(row.timestamp))
            .or_insert(row.timestamp);
    }

    let mut patterns: Vec<Pattern> = groups
        .into_iter()
        .filter(|(_, sessions)| sessions.len() as i64 >= threshold)
        .map(|(file_path, sessions)| {
            let heat: f64 = sessions
                .values()
                .map(|ts| {
                    let age_hours = (now - ts) as f64 / 3600.0;
                    exp_decay(age_hours, half_life)
                })
                .sum();
            let session_count = sessions.len() as i64;
            let session_ids: Vec<String> = sessions.into_keys().collect();
            Pattern {
                kind: "unresolved_read",
                description: format!(
                    "`{}` read in {session_count} sessions, never edited",
                    short_path(&file_path)
                ),
                normalized: file_path.clone(),
                session_count,
                heat,
                sessions: session_ids,
                example: file_path,
            }
        })
        .collect();

    patterns.sort_by(|a, b| b.heat.partial_cmp(&a.heat).unwrap_or(std::cmp::Ordering::Equal));
    patterns.truncate(20);
    Ok(patterns)
}

/// Detect recurring error patterns from failed command responses across sessions.
fn detect_error_patterns(
    conn: &Connection,
    threshold: i64,
    half_life: f64,
) -> Result<Vec<Pattern>, NmemError> {
    let now = now_secs();

    // Fetch failed commands that have error responses in metadata.
    let mut stmt = conn.prepare(
        "SELECT json_extract(metadata, '$.response'), session_id, MAX(timestamp) as latest_ts
         FROM observations
         WHERE obs_type = 'command'
           AND json_extract(metadata, '$.failed') = 1
           AND json_extract(metadata, '$.response') IS NOT NULL
         GROUP BY json_extract(metadata, '$.response'), session_id",
    )?;

    struct Row {
        response: String,
        session_id: String,
        timestamp: i64,
    }

    let rows: Vec<Row> = stmt
        .query_map([], |row| {
            Ok(Row {
                response: row.get(0)?,
                session_id: row.get(1)?,
                timestamp: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    // Extract error signature from response text — first meaningful error line.
    let mut groups: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut examples: HashMap<String, String> = HashMap::new();
    for row in &rows {
        let sig = extract_error_signature(&row.response);
        if sig.is_empty() {
            continue;
        }
        examples.entry(sig.clone()).or_insert_with(|| {
            row.response.chars().take(200).collect()
        });
        groups
            .entry(sig)
            .or_default()
            .entry(row.session_id.clone())
            .and_modify(|ts| *ts = (*ts).max(row.timestamp))
            .or_insert(row.timestamp);
    }

    let mut patterns: Vec<Pattern> = groups
        .into_iter()
        .filter(|(_, sessions)| sessions.len() as i64 >= threshold)
        .map(|(sig, sessions)| {
            let heat: f64 = sessions
                .values()
                .map(|ts| {
                    let age_hours = (now - ts) as f64 / 3600.0;
                    exp_decay(age_hours, half_life)
                })
                .sum();
            let session_count = sessions.len() as i64;
            let session_ids: Vec<String> = sessions.into_keys().collect();
            let example = examples.get(&sig).cloned().unwrap_or_default();
            Pattern {
                kind: "recurring_error",
                description: format!("`{}` across {session_count} sessions", short_cmd(&sig)),
                normalized: sig,
                session_count,
                heat,
                sessions: session_ids,
                example,
            }
        })
        .collect();

    patterns.sort_by(|a, b| b.heat.partial_cmp(&a.heat).unwrap_or(std::cmp::Ordering::Equal));
    patterns.truncate(20);
    Ok(patterns)
}

/// Extract a normalized error signature from a response string.
/// Looks for common error patterns and returns a short canonical form.
fn extract_error_signature(response: &str) -> String {
    for line in response.lines() {
        let line = line.trim();
        // "command not found" variants
        if line.contains("not found") || line.contains("No such file") {
            return normalize_error_line(line);
        }
        // Exit code patterns
        if line.contains("exit code") || line.contains("exit status") {
            return normalize_error_line(line);
        }
        // Compilation errors
        if line.contains("error[E") || line.starts_with("error:") {
            return normalize_error_line(line);
        }
        // Permission denied
        if line.contains("Permission denied") || line.contains("EACCES") {
            return normalize_error_line(line);
        }
        // Connection errors
        if line.contains("Connection refused") || line.contains("ECONNREFUSED") {
            return normalize_error_line(line);
        }
    }
    // Fallback: first non-empty line, truncated
    response
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(normalize_error_line)
        .unwrap_or_default()
}

/// Normalize an error line: strip paths, PIDs, timestamps for grouping.
fn normalize_error_line(line: &str) -> String {
    let s = line.trim();
    // Truncate to reasonable length
    let s: String = s.chars().take(120).collect();
    s
}

/// Detect repeated session intents from summaries.
fn detect_repeated_intents(
    conn: &Connection,
    threshold: i64,
    half_life: f64,
) -> Result<Vec<Pattern>, NmemError> {
    let now = now_secs();

    // Fetch sessions with summaries that have intents.
    let mut stmt = conn.prepare(
        "SELECT id, started_at, json_extract(summary, '$.intent') as intent
         FROM sessions
         WHERE summary IS NOT NULL
           AND json_extract(summary, '$.intent') IS NOT NULL",
    )?;

    struct Row {
        session_id: String,
        started_at: i64,
        intent: String,
    }

    let rows: Vec<Row> = stmt
        .query_map([], |row| {
            Ok(Row {
                session_id: row.get(0)?,
                started_at: row.get(1)?,
                intent: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Group similar intents using keyword bags + Jaccard similarity.
    // Each intent becomes a set of keywords; intents with >= 0.5 overlap merge.
    let bags: Vec<(usize, Vec<String>)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i, intent_keywords(&r.intent)))
        .collect();

    // Greedy clustering: for each intent, find or create a cluster.
    let mut clusters: Vec<Vec<usize>> = Vec::new(); // cluster → row indices
    let mut assigned = vec![false; rows.len()];

    for (i, bag_i) in &bags {
        if assigned[*i] {
            continue;
        }
        let mut cluster = vec![*i];
        assigned[*i] = true;

        for (j, bag_j) in &bags {
            if assigned[*j] {
                continue;
            }
            if jaccard(bag_i, bag_j) >= 0.4 {
                cluster.push(*j);
                assigned[*j] = true;
            }
        }
        clusters.push(cluster);
    }

    let mut patterns: Vec<Pattern> = clusters
        .into_iter()
        .filter(|c| c.len() as i64 >= threshold)
        .map(|c| {
            let heat: f64 = c
                .iter()
                .map(|&i| {
                    let age_hours = (now - rows[i].started_at) as f64 / 3600.0;
                    exp_decay(age_hours, half_life)
                })
                .sum();
            let session_count = c.len() as i64;
            let sessions: Vec<String> = c.iter().map(|&i| rows[i].session_id.clone()).collect();
            // Use the most recent intent as the representative
            let rep = c
                .iter()
                .max_by_key(|&&i| rows[i].started_at)
                .copied()
                .unwrap_or(c[0]);
            let intent = rows[rep].intent.clone();
            Pattern {
                kind: "repeated_intent",
                description: format!("similar intent across {session_count} sessions"),
                normalized: short_intent(&intent),
                session_count,
                heat,
                sessions,
                example: intent,
            }
        })
        .collect();

    patterns.sort_by(|a, b| b.heat.partial_cmp(&a.heat).unwrap_or(std::cmp::Ordering::Equal));
    patterns.truncate(20);
    Ok(patterns)
}

pub const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "to", "of", "in", "for", "with", "on", "at", "by", "from",
    "is", "it", "this", "that", "be", "as", "are", "was", "were", "been", "do", "does", "did",
    "will", "would", "could", "should", "may", "might", "can", "has", "have", "had", "not", "no",
    "up", "out", "about", "into", "over", "after", "before",
];

/// Extract meaningful keywords from an intent string.
pub fn intent_keywords(intent: &str) -> Vec<String> {
    intent
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// Jaccard similarity between two keyword bags (as sorted unique sets).
pub fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let set_a: std::collections::HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: std::collections::HashSet<&str> = b.iter().map(|s| s.as_str()).collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Shorten an intent for display.
fn short_intent(s: &str) -> String {
    if s.len() > 80 {
        format!("{}...", &s[..80])
    } else {
        s.to_string()
    }
}

/// Paths that are read-only by nature — library docs, design docs, configs.
/// Reading these repeatedly without editing is expected, not a signal.
fn is_reference_path(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').collect();
    segments.iter().any(|s| {
        matches!(
            *s,
            "library" | "ADR" | "design" | "docs" | ".claude" | "node_modules"
        )
    })
}

/// Shorten a command for display (first 60 chars).
fn short_cmd(s: &str) -> String {
    if s.len() > 60 {
        format!("{}...", &s[..60])
    } else {
        s.to_string()
    }
}

/// Shorten a file path for display — keep last 2 components.
fn short_path(s: &str) -> String {
    let parts: Vec<&str> = s.rsplitn(3, '/').collect();
    if parts.len() >= 2 {
        format!("{}/{}", parts[1], parts[0])
    } else {
        s.to_string()
    }
}

pub fn detect_patterns(
    conn: &Connection,
    threshold: i64,
    half_life: f64,
) -> Result<Vec<Pattern>, NmemError> {
    let mut all = detect_failed_commands(conn, threshold, half_life)?;
    all.extend(detect_unresolved_reads(conn, threshold, half_life)?);
    all.extend(detect_recurring_errors(conn, threshold, half_life)?);
    all.extend(detect_repeated_intents(conn, threshold, half_life)?);
    normalize_heat(&mut all);
    Ok(all)
}

/// Wrapper name matching the public API style.
fn detect_recurring_errors(
    conn: &Connection,
    threshold: i64,
    half_life: f64,
) -> Result<Vec<Pattern>, NmemError> {
    detect_error_patterns(conn, threshold, half_life)
}

/// Normalize raw heat to 0–100 relative to the hottest pattern.
fn normalize_heat(patterns: &mut [Pattern]) {
    let max = patterns.iter().map(|p| p.heat).fold(0.0f64, f64::max);
    if max > 0.0 {
        for p in patterns.iter_mut() {
            p.heat = (p.heat / max * 100.0).round();
        }
    }
}

pub fn write_report(patterns: &[Pattern], output: &Path) -> Result<(), NmemError> {
    use std::fmt::Write;

    let now = chrono_date();
    let failed: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "failed_command").collect();
    let unresolved: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "unresolved_read").collect();
    let errors: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "recurring_error").collect();
    let intents: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "repeated_intent").collect();

    let mut md = String::new();
    writeln!(md, "# nmem learnings — detected {now}").unwrap();
    writeln!(md).unwrap();

    if failed.is_empty() && unresolved.is_empty() && errors.is_empty() && intents.is_empty() {
        writeln!(md, "No patterns detected above threshold.").unwrap();
    }

    // Cross-reference: find session overlap between intents and errors/failures.
    let confirmed = find_confirmed(&intents, &failed, &errors);
    if !confirmed.is_empty() {
        writeln!(md, "## Confirmed stuck loops ({} patterns)", confirmed.len()).unwrap();
        writeln!(md).unwrap();
        writeln!(md, "Intent + failure/error co-occurring in the same sessions:").unwrap();
        writeln!(md).unwrap();
        for (intent, corroborating) in &confirmed {
            writeln!(md, "### {} (heat: {})", intent.normalized, intent.heat as u32).unwrap();
            writeln!(md, "Intent: {}", intent.example).unwrap();
            writeln!(md, "Corroborated by: {}", corroborating.join(", ")).unwrap();
            writeln!(md, "Sessions: {}", format_sessions(&intent.sessions)).unwrap();
            writeln!(md).unwrap();
        }
    }

    if !failed.is_empty() {
        writeln!(md, "## Repeated failures ({} patterns)", failed.len()).unwrap();
        writeln!(md).unwrap();
        for p in &failed {
            writeln!(md, "### `{}` — {} sessions (heat: {})", short_cmd(&p.normalized), p.session_count, p.heat as u32).unwrap();
            writeln!(md, "Normalized: `{}`", p.normalized).unwrap();
            writeln!(md, "Sessions: {}", format_sessions(&p.sessions)).unwrap();
            writeln!(md, "Example: `{}`", p.example).unwrap();
            writeln!(md).unwrap();
        }
    }

    if !errors.is_empty() {
        writeln!(md, "## Recurring errors ({} patterns)", errors.len()).unwrap();
        writeln!(md).unwrap();
        for p in &errors {
            writeln!(md, "### `{}` — {} sessions (heat: {})", short_cmd(&p.normalized), p.session_count, p.heat as u32).unwrap();
            writeln!(md, "Sessions: {}", format_sessions(&p.sessions)).unwrap();
            writeln!(md, "Example: `{}`", p.example).unwrap();
            writeln!(md).unwrap();
        }
    }

    if !intents.is_empty() {
        writeln!(md, "## Repeated intents ({} patterns)", intents.len()).unwrap();
        writeln!(md).unwrap();
        for p in &intents {
            writeln!(md, "### {} — {} sessions (heat: {})", p.normalized, p.session_count, p.heat as u32).unwrap();
            writeln!(md, "Sessions: {}", format_sessions(&p.sessions)).unwrap();
            writeln!(md, "Example: {}", p.example).unwrap();
            writeln!(md).unwrap();
        }
    }

    if !unresolved.is_empty() {
        writeln!(md, "## Unresolved investigations ({} patterns)", unresolved.len()).unwrap();
        writeln!(md).unwrap();
        for p in &unresolved {
            writeln!(md, "### `{}` — {} sessions (heat: {})", short_path(&p.normalized), p.session_count, p.heat as u32).unwrap();
            writeln!(md, "Read in {} sessions, never edited.", p.session_count).unwrap();
            writeln!(md, "Sessions: {}", format_sessions(&p.sessions)).unwrap();
            writeln!(md).unwrap();
        }
    }

    if let Some(parent) = output.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, md)?;
    Ok(())
}

/// Find intents that share sessions with failures or errors — confirmed stuck loops.
fn find_confirmed<'a>(
    intents: &[&'a Pattern],
    failures: &[&Pattern],
    errors: &[&Pattern],
) -> Vec<(&'a Pattern, Vec<String>)> {
    use std::collections::HashSet;

    let mut confirmed = Vec::new();
    for intent in intents {
        let intent_sessions: HashSet<&str> = intent.sessions.iter().map(|s| s.as_str()).collect();
        let mut corroborating = Vec::new();

        for f in failures {
            let f_sessions: HashSet<&str> = f.sessions.iter().map(|s| s.as_str()).collect();
            let overlap = intent_sessions.intersection(&f_sessions).count();
            if overlap >= 2 {
                corroborating.push(format!("failure `{}`", short_cmd(&f.normalized)));
            }
        }
        for e in errors {
            let e_sessions: HashSet<&str> = e.sessions.iter().map(|s| s.as_str()).collect();
            let overlap = intent_sessions.intersection(&e_sessions).count();
            if overlap >= 2 {
                corroborating.push(format!("error `{}`", short_cmd(&e.normalized)));
            }
        }

        if !corroborating.is_empty() {
            confirmed.push((*intent, corroborating));
        }
    }
    confirmed
}

fn format_sessions(sessions: &[String]) -> String {
    sessions
        .iter()
        .map(|s| {
            if s.len() > 8 {
                &s[..8]
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn chrono_date() -> String {
    // Simple date without pulling in chrono — just use the system
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Convert unix timestamp to YYYY-MM-DD
    let days = now / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let months = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 1;
    for &days_in_month in &months {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        m += 1;
    }
    let d = remaining + 1;
    format!("{y:04}-{m:02}-{d:02}")
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn default_output() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".nmem").join("learnings.md")
}

pub fn handle_learn(db_path: &Path, args: &LearnArgs) -> Result<(), NmemError> {
    let conn = open_db_readonly(db_path)?;
    let patterns = detect_patterns(&conn, args.threshold, args.half_life)?;
    let output = args.output.clone().unwrap_or_else(default_output);

    write_report(&patterns, &output)?;

    let failed_count = patterns.iter().filter(|p| p.kind == "failed_command").count();
    let error_count = patterns.iter().filter(|p| p.kind == "recurring_error").count();
    let intent_count = patterns.iter().filter(|p| p.kind == "repeated_intent").count();
    let unresolved_count = patterns.iter().filter(|p| p.kind == "unresolved_read").count();

    eprintln!(
        "nmem: {failed_count} failures, {error_count} errors, {intent_count} intents, {unresolved_count} unresolved → {}",
        output.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MIGRATIONS;

    fn setup_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn insert_session(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES (?1, 'test', 1000)",
            [id],
        )
        .unwrap();
    }

    fn insert_obs(
        conn: &Connection,
        session_id: &str,
        obs_type: &str,
        content: &str,
        file_path: Option<&str>,
        metadata: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO observations (session_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
             VALUES (?1, 1000, ?2, 'PostToolUse', 'Bash', ?3, ?4, ?5)",
            rusqlite::params![session_id, obs_type, file_path, content, metadata],
        )
        .unwrap();
    }

    #[test]
    fn detects_repeated_failed_commands() {
        let conn = setup_db();
        for i in 0..4 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
            insert_obs(
                &conn,
                &sid,
                "command",
                "cargo test 2>&1",
                None,
                Some(r#"{"failed": true}"#),
            );
        }

        let patterns = detect_patterns(&conn, 3, 168.0).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, "failed_command");
        assert_eq!(patterns[0].session_count, 4);
    }

    #[test]
    fn below_threshold_returns_empty() {
        let conn = setup_db();
        for i in 0..2 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
            insert_obs(
                &conn,
                &sid,
                "command",
                "cargo test",
                None,
                Some(r#"{"failed": true}"#),
            );
        }

        let patterns = detect_patterns(&conn, 3, 168.0).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn normalization_groups_variants() {
        let conn = setup_db();
        for i in 0..3 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
        }
        // Variant 1: with path prefix + 2>&1
        insert_obs(&conn, "session-0", "command", "~/.cargo/bin/cargo test 2>&1", None, Some(r#"{"failed": true}"#));
        // Variant 2: plain
        insert_obs(&conn, "session-1", "command", "cargo test", None, Some(r#"{"failed": true}"#));
        // Variant 3: with 2>&1 only
        insert_obs(&conn, "session-2", "command", "cargo test 2>&1", None, Some(r#"{"failed": true}"#));

        let patterns = detect_failed_commands(&conn, 3, 168.0).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].session_count, 3);
        assert_eq!(patterns[0].normalized, "cargo test");
    }

    #[test]
    fn detects_unresolved_reads() {
        let conn = setup_db();
        for i in 0..3 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
            insert_obs(
                &conn,
                &sid,
                "file_read",
                "read content",
                Some("/src/mystery.rs"),
                None,
            );
        }

        let patterns = detect_patterns(&conn, 3, 168.0).unwrap();
        let reads: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "unresolved_read").collect();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].session_count, 3);
        assert!(reads[0].normalized.contains("mystery.rs"));
    }

    #[test]
    fn edited_files_excluded_from_unresolved() {
        let conn = setup_db();
        for i in 0..3 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
            insert_obs(&conn, &sid, "file_read", "read", Some("/src/fixed.rs"), None);
        }
        // File was edited in one session — should exclude it
        insert_obs(&conn, "session-0", "file_edit", "edit", Some("/src/fixed.rs"), None);

        let patterns = detect_patterns(&conn, 3, 168.0).unwrap();
        let reads: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "unresolved_read").collect();
        assert!(reads.is_empty());
    }

    #[test]
    fn recent_observations_score_higher_heat() {
        let conn = setup_db();
        let now = now_secs();

        // "hot" file — read in 3 sessions in the last hour
        for i in 0..3 {
            let sid = format!("hot-{i}");
            insert_session(&conn, &sid);
            conn.execute(
                "INSERT INTO observations (session_id, timestamp, obs_type, source_event, tool_name, file_path, content)
                 VALUES (?1, ?2, 'file_read', 'PostToolUse', 'Read', '/src/hot.rs', 'content')",
                rusqlite::params![sid, now - (i * 600)], // 0, 10, 20 minutes ago
            ).unwrap();
        }

        // "cold" file — read in 3 sessions 30 days ago
        for i in 0..3 {
            let sid = format!("cold-{i}");
            insert_session(&conn, &sid);
            conn.execute(
                "INSERT INTO observations (session_id, timestamp, obs_type, source_event, tool_name, file_path, content)
                 VALUES (?1, ?2, 'file_read', 'PostToolUse', 'Read', '/src/cold.rs', 'content')",
                rusqlite::params![sid, now - (30 * 86400) - (i * 600)],
            ).unwrap();
        }

        let patterns = detect_patterns(&conn, 3, 168.0).unwrap();
        let reads: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "unresolved_read").collect();
        assert_eq!(reads.len(), 2);
        // Hot file should rank first (heat 100 after normalization)
        assert!(reads[0].normalized.contains("hot.rs"));
        assert!(reads[1].normalized.contains("cold.rs"));
        assert!(reads[0].heat > reads[1].heat);
        // Hot = 100, cold should be very low (30 days with 7-day half-life ≈ 5%)
        assert_eq!(reads[0].heat as u32, 100);
        assert!(reads[1].heat <= 10.0, "cold heat was {}", reads[1].heat);
    }

    #[test]
    fn write_report_produces_markdown() {
        let patterns = vec![
            Pattern {
                kind: "failed_command",
                description: "`cargo test` failed across 4 sessions".into(),
                normalized: "cargo test".into(),
                session_count: 4,
                heat: 100.0,
                sessions: vec!["aaa".into(), "bbb".into(), "ccc".into(), "ddd".into()],
                example: "~/.cargo/bin/cargo test 2>&1".into(),
            },
            Pattern {
                kind: "unresolved_read",
                description: "`src/mystery.rs` read in 3 sessions, never edited".into(),
                normalized: "/home/user/src/mystery.rs".into(),
                session_count: 3,
                heat: 38.0,
                sessions: vec!["aaa".into(), "bbb".into(), "ccc".into()],
                example: "/home/user/src/mystery.rs".into(),
            },
        ];

        let dir = tempfile::TempDir::new().unwrap();
        let output = dir.path().join("learnings.md");
        write_report(&patterns, &output).unwrap();

        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("# nmem learnings"));
        assert!(content.contains("## Repeated failures (1 patterns)"));
        assert!(content.contains("## Unresolved investigations (1 patterns)"));
        assert!(content.contains("cargo test"));
        assert!(content.contains("heat: 100)"));
        assert!(content.contains("heat: 38)"));
        assert!(content.contains("mystery.rs"));
    }

    #[test]
    fn normalize_strips_redirects_and_prefixes() {
        assert_eq!(normalize_command("cargo test 2>&1"), "cargo test");
        assert_eq!(normalize_command("~/.cargo/bin/cargo test"), "cargo test");
        assert_eq!(
            normalize_command("cargo test 2>&1 | tail -5"),
            "cargo test"
        );
        assert_eq!(
            normalize_command("/home/bpd/workspace/foo 2>&1"),
            "workspace/foo"
        );
    }

    #[test]
    fn normalize_collapses_subcommands() {
        assert_eq!(normalize_command("cargo test s3_learn"), "cargo test");
        assert_eq!(normalize_command("cargo build --release"), "cargo build");
        assert_eq!(normalize_command("cargo test -- --list"), "cargo test");
        // Two-token commands stay as-is
        assert_eq!(normalize_command("cargo test"), "cargo test");
        // Non-subcommand tools keep full string
        assert_eq!(normalize_command("ls -la /tmp"), "ls -la /tmp");
    }

    #[test]
    fn diagnostic_commands_filtered() {
        assert!(is_diagnostic("which cargo"));
        assert!(is_diagnostic("tmux kill-window -t nmem:task-3"));
        assert!(is_diagnostic("sleep 30 && tmux capture-pane -t foo -p"));
        assert!(is_diagnostic("source ~/.zshrc 2>/dev/null; cargo test"));
        assert!(is_diagnostic("export PATH=\"/foo\" && cargo test"));
        assert!(!is_diagnostic("cargo test"));
        assert!(!is_diagnostic("cargo build"));
    }

    #[test]
    fn detects_recurring_error_patterns() {
        let conn = setup_db();
        for i in 0..3 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
            insert_obs(
                &conn,
                &sid,
                "command",
                "cargo test",
                None,
                Some(r#"{"failed": true, "response": "error: cargo: command not found"}"#),
            );
        }

        let patterns = detect_error_patterns(&conn, 3, 168.0).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, "recurring_error");
        assert_eq!(patterns[0].session_count, 3);
        assert!(patterns[0].normalized.contains("not found"));
    }

    #[test]
    fn error_patterns_below_threshold_empty() {
        let conn = setup_db();
        for i in 0..2 {
            let sid = format!("session-{i}");
            insert_session(&conn, &sid);
            insert_obs(
                &conn,
                &sid,
                "command",
                "cargo test",
                None,
                Some(r#"{"failed": true, "response": "error: cargo: command not found"}"#),
            );
        }

        let patterns = detect_error_patterns(&conn, 3, 168.0).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn detects_repeated_intents() {
        let conn = setup_db();
        for i in 0..3 {
            let sid = format!("session-{i}");
            conn.execute(
                "INSERT INTO sessions (id, project, started_at, summary) VALUES (?1, 'test', ?2, ?3)",
                rusqlite::params![
                    sid,
                    1000 + i * 3600,
                    format!(r#"{{"intent": "fix cargo test PATH issue in dispatched sessions"}}"#),
                ],
            ).unwrap();
        }

        let patterns = detect_repeated_intents(&conn, 3, 168.0).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].kind, "repeated_intent");
        assert_eq!(patterns[0].session_count, 3);
    }

    #[test]
    fn similar_intents_cluster_together() {
        let conn = setup_db();
        let intents = [
            "fix cargo test PATH issue in tmux dispatch",
            "fix cargo test PATH problem in dispatched sessions",
            "debug cargo test PATH failure in task dispatch",
        ];
        for (i, intent) in intents.iter().enumerate() {
            let sid = format!("session-{i}");
            conn.execute(
                "INSERT INTO sessions (id, project, started_at, summary) VALUES (?1, 'test', ?2, ?3)",
                rusqlite::params![sid, 1000 + i as i64 * 3600, format!(r#"{{"intent": "{intent}"}}"#)],
            ).unwrap();
        }

        let patterns = detect_repeated_intents(&conn, 3, 168.0).unwrap();
        assert_eq!(patterns.len(), 1, "similar intents should cluster into 1 pattern");
        assert_eq!(patterns[0].session_count, 3);
    }

    #[test]
    fn dissimilar_intents_stay_separate() {
        let conn = setup_db();
        let intents = [
            "fix cargo test PATH issue",
            "add new MCP server endpoint for search",
            "refactor database schema migrations",
        ];
        for (i, intent) in intents.iter().enumerate() {
            let sid = format!("session-{i}");
            conn.execute(
                "INSERT INTO sessions (id, project, started_at, summary) VALUES (?1, 'test', ?2, ?3)",
                rusqlite::params![sid, 1000 + i as i64 * 3600, format!(r#"{{"intent": "{intent}"}}"#)],
            ).unwrap();
        }

        // With threshold 3, none should qualify since they're all different
        let patterns = detect_repeated_intents(&conn, 3, 168.0).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn jaccard_similarity_basic() {
        let a: Vec<String> = vec!["cargo".into(), "test".into(), "path".into()];
        let b: Vec<String> = vec!["cargo".into(), "test".into(), "fix".into()];
        let sim = jaccard(&a, &b);
        // intersection=2, union=4 → 0.5
        assert!((sim - 0.5).abs() < 0.01);

        // Identical sets → 1.0
        assert!((jaccard(&a, &a) - 1.0).abs() < 0.01);

        // Disjoint → 0.0
        let c: Vec<String> = vec!["database".into(), "schema".into()];
        assert!((jaccard(&a, &c) - 0.0).abs() < 0.01);
    }

    #[test]
    fn intent_keywords_strips_stopwords() {
        let kw = intent_keywords("fix the cargo test PATH issue in dispatched sessions");
        assert!(kw.contains(&"fix".to_string()));
        assert!(kw.contains(&"cargo".to_string()));
        assert!(kw.contains(&"test".to_string()));
        assert!(kw.contains(&"path".to_string()));
        assert!(!kw.contains(&"the".to_string()));
        assert!(!kw.contains(&"in".to_string()));
    }

    #[test]
    fn extract_error_signature_finds_not_found() {
        let sig = extract_error_signature("bash: cargo: command not found\nexit code 127");
        assert!(sig.contains("not found"));
    }

    #[test]
    fn extract_error_signature_finds_compilation_error() {
        let sig = extract_error_signature("warning: unused variable\nerror[E0433]: failed to resolve");
        assert!(sig.contains("error[E"));
    }

    #[test]
    fn confirmed_stuck_loops_detected() {
        let conn = setup_db();
        let shared_sessions = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];

        // Set up sessions with intents
        for sid in &shared_sessions {
            conn.execute(
                "INSERT INTO sessions (id, project, started_at, summary) VALUES (?1, 'test', 1000, ?2)",
                rusqlite::params![sid, r#"{"intent": "fix cargo test PATH issue"}"#],
            ).unwrap();
            insert_obs(&conn, sid, "command", "cargo test", None,
                Some(r#"{"failed": true, "response": "cargo: command not found"}"#));
        }

        let patterns = detect_patterns(&conn, 3, 168.0).unwrap();
        let intents: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "repeated_intent").collect();
        let failures: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "failed_command").collect();
        let errors: Vec<&Pattern> = patterns.iter().filter(|p| p.kind == "recurring_error").collect();

        let confirmed = find_confirmed(&intents, &failures, &errors);
        assert!(!confirmed.is_empty(), "should detect confirmed stuck loop");
        assert!(!confirmed[0].1.is_empty(), "should have corroborating evidence");
    }
}
