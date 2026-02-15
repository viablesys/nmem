use crate::config::{load_config, resolve_filter_params, NmemConfig};
use crate::context;
use crate::db::open_db;
use crate::extract::{classify_tool, extract_content, extract_file_path};
use crate::filter::{SecretFilter, redact_json_value_with};
use crate::project::derive_project;
use crate::sweep::run_sweep;
use crate::transcript::{get_current_prompt_id, scan_transcript};
use crate::NmemError;
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
struct HookPayload {
    session_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Option<serde_json::Value>,
    #[serde(default)]
    transcript_path: Option<String>,
    // SessionStart specific
    #[serde(default)]
    source: Option<String>,
    // UserPromptSubmit specific
    #[serde(default)]
    prompt: Option<String>,
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn ensure_session(conn: &Connection, session_id: &str, cwd: &str, ts: i64) -> Result<(), NmemError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
        params![session_id],
        |r| r.get(0),
    )?;

    if !exists {
        let project = derive_project(cwd);
        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES (?1, ?2, ?3)",
            params![session_id, project, ts],
        )?;
    }

    Ok(())
}

fn maybe_sweep(conn: &Connection, config: &NmemConfig) {
    if !config.retention.enabled {
        return;
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE timestamp < unixepoch('now') - 86400",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if count < 100 {
        return;
    }
    match run_sweep(conn, &config.retention) {
        Ok(r) if r.deleted > 0 => {
            eprintln!("nmem: sweep deleted {} expired observations", r.deleted)
        }
        Err(e) => eprintln!("nmem: sweep error (non-fatal): {e}"),
        _ => {}
    }
}

fn handle_session_start(
    conn: &Connection,
    payload: &HookPayload,
    config: &NmemConfig,
    project: &str,
) -> Result<(), NmemError> {
    let ts = now_ts();
    let tx = conn.unchecked_transaction()?;

    ensure_session(&tx, &payload.session_id, &payload.cwd, ts)?;

    let source = payload.source.as_deref().unwrap_or("startup");
    if matches!(source, "compact" | "resume" | "clear") {
        let prompt_id = get_current_prompt_id(&tx, &payload.session_id)?;
        tx.execute(
            "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                payload.session_id,
                prompt_id,
                ts,
                format!("session_{source}"),
                "SessionStart",
                source,
            ],
        )?;
    }

    tx.commit()?;

    maybe_sweep(conn, config);

    // Context injection — non-fatal, errors logged to stderr
    match context::generate_context(conn, project, source) {
        Ok(ctx) if !ctx.is_empty() => print!("{ctx}"),
        Ok(_) => {}
        Err(e) => eprintln!("nmem: context injection failed: {e}"),
    }

    Ok(())
}

fn handle_user_prompt(
    conn: &Connection,
    payload: &HookPayload,
    filter: &SecretFilter,
) -> Result<(), NmemError> {
    let prompt = match payload.prompt.as_deref() {
        Some(p) if !p.is_empty() && !p.starts_with("<system-reminder>") => p,
        _ => return Ok(()),
    };

    let ts = now_ts();
    let tx = conn.unchecked_transaction()?;

    ensure_session(&tx, &payload.session_id, &payload.cwd, ts)?;

    // Truncate and filter secrets
    let truncated: String = prompt.chars().take(500).collect();
    let (filtered, redacted) = filter.redact(&truncated);

    if redacted {
        eprintln!("nmem: redacted potential secret from user_prompt");
    }

    tx.execute(
        "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?1, ?2, ?3, ?4)",
        params![payload.session_id, ts, "user", filtered],
    )?;

    tx.commit()?;
    Ok(())
}

fn handle_post_tool_use(
    conn: &Connection,
    payload: &HookPayload,
    filter: &SecretFilter,
) -> Result<(), NmemError> {
    let tool_name = match payload.tool_name.as_deref() {
        Some(n) => n,
        None => return Ok(()),
    };
    let tool_input = payload
        .tool_input
        .as_ref()
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let ts = now_ts();
    let tx = conn.unchecked_transaction()?;

    ensure_session(&tx, &payload.session_id, &payload.cwd, ts)?;

    // Scan transcript for thinking blocks
    let prompt_id = if let Some(tp) = payload.transcript_path.as_deref() {
        scan_transcript(&tx, &payload.session_id, tp, ts)?
    } else {
        get_current_prompt_id(&tx, &payload.session_id)?
    };

    let obs_type = classify_tool(tool_name);
    let content = extract_content(tool_name, &tool_input);
    let file_path = extract_file_path(tool_name, &tool_input);

    // Filter secrets from content
    let (filtered_content, content_redacted) = filter.redact(&content);

    // Build metadata and filter it
    let mut metadata = serde_json::Value::Null;
    if content_redacted {
        metadata = serde_json::json!({"redacted": true});
    }

    // Filter secrets from metadata if it has content
    if metadata.is_object() {
        redact_json_value_with(&mut metadata, filter);
    }

    let metadata_str = if metadata.is_null() {
        None
    } else {
        Some(serde_json::to_string(&metadata)?)
    };

    if content_redacted {
        eprintln!("nmem: redacted potential secret from {obs_type} observation");
    }

    tx.execute(
        "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            payload.session_id,
            prompt_id,
            ts,
            obs_type,
            "PostToolUse",
            tool_name,
            file_path,
            filtered_content,
            metadata_str,
        ],
    )?;

    tx.commit()?;
    Ok(())
}

fn handle_stop(conn: &Connection, payload: &HookPayload) -> Result<(), NmemError> {
    let ts = now_ts();
    let tx = conn.unchecked_transaction()?;

    // Final transcript scan
    if let Some(tp) = payload.transcript_path.as_deref() {
        scan_transcript(&tx, &payload.session_id, tp, ts)?;
    }

    // Compute session signature (scope stmt so borrow is dropped before commit)
    let sig_json = {
        let mut stmt = tx.prepare(
            "SELECT obs_type, COUNT(*) as n FROM observations
             WHERE session_id = ?1 GROUP BY obs_type ORDER BY n DESC",
        )?;
        let signature: Vec<(String, i64)> = stmt
            .query_map(params![payload.session_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<Result<_, _>>()?;

        if signature.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&signature)?)
        }
    };

    tx.execute(
        "UPDATE sessions SET ended_at = ?1, signature = ?2 WHERE id = ?3",
        params![ts, sig_json, payload.session_id],
    )?;

    tx.commit()?;

    // WAL checkpoint outside transaction
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;

    Ok(())
}

pub fn handle_record(db_path: &Path) -> Result<(), NmemError> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let payload: HookPayload = serde_json::from_str(&input)?;

    if payload.session_id.is_empty() {
        return Ok(());
    }

    let conn = open_db(db_path)?;

    // Load config and create project-aware filter
    let config = load_config().unwrap_or_default();
    let project = derive_project(&payload.cwd);
    let params = resolve_filter_params(&config, Some(&project));
    let filter = SecretFilter::with_params(params);

    match payload.hook_event_name.as_str() {
        "SessionStart" => handle_session_start(&conn, &payload, &config, &project),
        "UserPromptSubmit" => handle_user_prompt(&conn, &payload, &filter),
        "PostToolUse" => handle_post_tool_use(&conn, &payload, &filter),
        "Stop" => handle_stop(&conn, &payload),
        _ => Ok(()),
    }
}
