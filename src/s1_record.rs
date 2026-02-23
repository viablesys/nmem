use crate::s4_context;
use crate::s1_extract::{classify_tool, extract_content, extract_file_path, extract_git_metadata};
use crate::s1_4_transcript::{get_current_prompt_id, scan_transcript};
use crate::s2_classify;
use crate::s2_locus;
use crate::s2_novelty;
use crate::s2_scope;
use crate::s3_sweep::run_sweep;
use crate::s5_config::{load_config, resolve_filter_params, NmemConfig};
use crate::s5_filter::{SecretFilter, redact_json_value_with};
use crate::s5_project::derive_project;
use crate::db::open_db;
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
    tool_response: Option<serde_json::Value>,
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

fn maybe_sweep(conn: &Connection, config: &NmemConfig, db_path: &Path) {
    if !config.retention.enabled {
        return;
    }

    // Size-based trigger: check if DB + WAL exceeds configured limit
    let size_exceeded = config.retention.max_db_size_mb.is_some_and(|limit| {
        let db_size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        let wal_size = std::fs::metadata(db_path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
        (db_size + wal_size) / (1024 * 1024) > limit as u64
    });

    // Count-based trigger: enough old observations to justify a sweep
    let count_exceeded = conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE timestamp < unixepoch('now') - 86400",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        >= 100;

    if !size_exceeded && !count_exceeded {
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

    // Context injection — non-fatal, errors logged to stderr
    let is_recovery = matches!(source, "compact" | "clear");
    let (local_limit, cross_limit) = crate::s5_config::resolve_context_limits(config, project, is_recovery);
    match s4_context::generate_context(conn, project, local_limit, cross_limit, None) {
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
    source_event: &str,
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

    let content = extract_content(tool_name, &tool_input);
    let obs_type = if tool_name == "Bash" {
        crate::s1_extract::classify_bash(&content)
    } else {
        classify_tool(tool_name)
    };
    let file_path = extract_file_path(tool_name, &tool_input);

    // Filter secrets from content
    let (filtered_content, content_redacted) = filter.redact(&content);

    // Build metadata and filter it
    let is_failure = source_event == "PostToolUseFailure";
    let mut meta_obj = serde_json::Map::new();

    if content_redacted {
        meta_obj.insert("redacted".into(), serde_json::Value::Bool(true));
    }

    // Extract tool_response as string (used by failure capture and git metadata)
    let response_str = payload.tool_response.as_ref().map(|resp| match resp {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    });

    if is_failure {
        meta_obj.insert("failed".into(), serde_json::Value::Bool(true));
        if let Some(ref resp) = response_str {
            let truncated: String = resp.chars().take(2000).collect();
            let (filtered_resp, _) = filter.redact(&truncated);
            meta_obj.insert("response".into(), serde_json::Value::String(filtered_resp));
        }
    }

    // Extract structured git metadata from tool_response
    if matches!(obs_type, "git_commit" | "git_push")
        && let Some(ref resp) = response_str {
            let git_meta = extract_git_metadata(obs_type, resp);
            for (k, v) in git_meta {
                meta_obj.insert(k, v);
            }
        }

    let mut metadata = if meta_obj.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(meta_obj)
    };

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

    // Classify phase (think/act) — non-fatal, None if model not loaded
    let phase_result = s2_classify::classify(&filtered_content);
    let phase = phase_result.as_ref().map(|p| p.label);

    // Register classifier run for provenance tracking
    let classifier_run_id = phase_result
        .as_ref()
        .and_then(|p| {
            s2_classify::ensure_classifier_run(&tx, "think-act", p.model_hash, None, None, None).ok()
        });

    // Classify scope (converge/diverge) — non-fatal, None if model not loaded
    let scope_result = s2_scope::classify_scope(&filtered_content);
    let scope = scope_result.as_ref().map(|s| s.label);
    let scope_run_id = scope_result.as_ref().and_then(|s| {
        s2_classify::ensure_classifier_run(&tx, "converge-diverge", s.model_hash, None, None, None)
            .ok()
    });

    // Classify locus (internal/external) — non-fatal
    let locus_result = s2_locus::classify_locus(&filtered_content);
    let locus = locus_result.as_ref().map(|r| r.label);
    let locus_run_id = locus_result.as_ref().and_then(|r| {
        s2_classify::ensure_classifier_run(&tx, "internal-external", r.model_hash, None, None, None).ok()
    });

    // Classify novelty (routine/novel) — non-fatal
    let novelty_result = s2_novelty::classify_novelty(&filtered_content);
    let novelty = novelty_result.as_ref().map(|r| r.label);
    let novelty_run_id = novelty_result.as_ref().and_then(|r| {
        s2_classify::ensure_classifier_run(&tx, "routine-novel", r.model_hash, None, None, None).ok()
    });

    // Friction is now computed at episode level (S4), not per-observation
    let friction: Option<&str> = None;
    let friction_run_id: Option<i64> = None;

    tx.execute(
        "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, tool_name, file_path, content, metadata, phase, classifier_run_id, scope, scope_run_id, locus, locus_run_id, novelty, novelty_run_id, friction, friction_run_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            payload.session_id,
            prompt_id,
            ts,
            obs_type,
            source_event,
            tool_name,
            file_path,
            filtered_content,
            metadata_str,
            phase,
            classifier_run_id,
            scope,
            scope_run_id,
            locus,
            locus_run_id,
            novelty,
            novelty_run_id,
            friction,
            friction_run_id,
        ],
    )?;

    tx.commit()?;

    // Stream to VictoriaLogs — non-fatal, fire-and-forget
    stream_observation_to_logs(
        &payload.session_id,
        &derive_project(&payload.cwd),
        obs_type,
        tool_name,
        file_path.as_deref(),
        &filtered_content,
        phase,
        scope,
        locus,
        novelty,
        friction,
        &metadata_str,
    );

    Ok(())
}

const VLOGS_ENDPOINT: &str = "http://localhost:9428/insert/jsonline";

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_observation_to_logs(
    session_id: &str,
    project: &str,
    obs_type: &str,
    tool_name: &str,
    file_path: Option<&str>,
    content: &str,
    phase: Option<&str>,
    scope: Option<&str>,
    locus: Option<&str>,
    novelty: Option<&str>,
    friction: Option<&str>,
    metadata_str: &Option<String>,
) {
    // Build a meaningful _msg — for git ops, use commit info instead of raw command
    let msg = build_log_message(obs_type, file_path, content, metadata_str);

    let mut record = serde_json::json!({
        "_msg": msg,
        "service": "nmem",
        "type": "observation",
        "session_id": session_id,
        "project": project,
        "obs_type": obs_type,
        "tool_name": tool_name,
    });

    if let Some(fp) = file_path {
        record["file_path"] = serde_json::Value::String(fp.to_string());
    }
    if let Some(p) = phase {
        record["phase"] = serde_json::Value::String(p.to_string());
    }
    if let Some(s) = scope {
        record["scope"] = serde_json::Value::String(s.to_string());
    }
    if let Some(l) = locus {
        record["locus"] = serde_json::Value::String(l.to_string());
    }
    if let Some(n) = novelty {
        record["novelty"] = serde_json::Value::String(n.to_string());
    }
    if let Some(fr) = friction {
        record["friction"] = serde_json::Value::String(fr.to_string());
    }

    // Merge git-specific fields into the log record
    if matches!(obs_type, "git_commit" | "git_push")
        && let Some(ms) = metadata_str
            && let Ok(meta) = serde_json::from_str::<serde_json::Value>(ms) {
                for key in &["commit_hash", "commit_message", "branch", "files_changed",
                             "insertions", "deletions", "remote_url", "hash_range"] {
                    if let Some(v) = meta.get(*key) {
                        record[*key] = v.clone();
                    }
                }
            }

    let body = format!("{}\n", record);
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(1)))
            .build(),
    );
    let _ = agent
        .post(VLOGS_ENDPOINT)
        .header("Content-Type", "application/stream+json")
        .send(body.as_bytes());
}

fn build_log_message(
    obs_type: &str,
    file_path: Option<&str>,
    content: &str,
    metadata_str: &Option<String>,
) -> String {
    // For git commits, show "[hash] message" instead of the raw command
    if obs_type == "git_commit"
        && let Some(ms) = metadata_str
            && let Ok(meta) = serde_json::from_str::<serde_json::Value>(ms) {
                let hash = meta.get("commit_hash").and_then(|v| v.as_str()).unwrap_or("?");
                let msg = meta.get("commit_message").and_then(|v| v.as_str()).unwrap_or("");
                let stats = format!(
                    "{}+/{}−",
                    meta.get("insertions").and_then(|v| v.as_u64()).unwrap_or(0),
                    meta.get("deletions").and_then(|v| v.as_u64()).unwrap_or(0),
                );
                return format!("git_commit: [{hash}] {msg} ({stats})");
            }
    if obs_type == "git_push"
        && let Some(ms) = metadata_str
            && let Ok(meta) = serde_json::from_str::<serde_json::Value>(ms) {
                let range = meta.get("hash_range").and_then(|v| v.as_str()).unwrap_or("?");
                let branch = meta.get("branch").and_then(|v| v.as_str()).unwrap_or("?");
                let remote = meta.get("remote_url").and_then(|v| v.as_str()).unwrap_or("?");
                return format!("git_push: {range} {branch} → {remote}");
            }
    // Default: file_path or content preview
    if let Some(fp) = file_path {
        format!("{obs_type}: {fp}")
    } else {
        let preview: String = content.chars().take(80).collect();
        format!("{obs_type}: {preview}")
    }
}

fn handle_stop(conn: &Connection, payload: &HookPayload, config: &NmemConfig, db_path: &Path) -> Result<(), NmemError> {
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

    // Detect episodes — non-fatal
    match crate::s4_memory::detect_and_narrate_episodes(conn, &payload.session_id, &config.summarization) {
        Ok(n) if n > 1 => eprintln!("nmem: {n} episodes detected"),
        Err(e) => eprintln!("nmem: episode detection failed (non-fatal): {e}"),
        _ => {}
    }

    // Summarize session — non-fatal
    match crate::s1_4_summarize::summarize_session(conn, &payload.session_id, &config.summarization) {
        Ok(()) => {}
        Err(e) => eprintln!("nmem: summarization failed (non-fatal): {e}"),
    }

    // Retention sweep — non-fatal
    maybe_sweep(conn, config, db_path);

    // WAL checkpoint outside transaction
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;

    Ok(())
}

pub fn handle_record(db_path: &Path) -> Result<(), NmemError> {
    let start = std::time::Instant::now();

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

    let result = match payload.hook_event_name.as_str() {
        "SessionStart" => handle_session_start(&conn, &payload, &config, &project),
        "UserPromptSubmit" => handle_user_prompt(&conn, &payload, &filter),
        "PostToolUse" => handle_post_tool_use(&conn, &payload, &filter, "PostToolUse"),
        "PostToolUseFailure" => handle_post_tool_use(&conn, &payload, &filter, "PostToolUseFailure"),
        "Stop" => handle_stop(&conn, &payload, &config, db_path),
        _ => Ok(()),
    };

    // Metrics export — non-fatal
    if config.metrics.enabled {
        record_metrics(&config, &payload, &project, result.is_ok(), start);
    }

    result
}

fn record_metrics(
    config: &NmemConfig,
    payload: &HookPayload,
    project: &str,
    success: bool,
    start: std::time::Instant,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("nmem: metrics runtime: {e}");
            return;
        }
    };

    let provider = match rt.block_on(async {
        crate::metrics::init_meter_provider(&config.metrics)
    }) {
        Some(p) => p,
        None => return,
    };

    let meter = opentelemetry::global::meter("nmem");

    if success {
        use opentelemetry::KeyValue;
        match payload.hook_event_name.as_str() {
            "SessionStart" => {
                meter
                    .u64_counter("nmem_sessions_total")
                    .build()
                    .add(1, &[KeyValue::new("project", project.to_string())]);
            }
            "UserPromptSubmit" => {
                meter
                    .u64_counter("nmem_prompts_total")
                    .build()
                    .add(1, &[KeyValue::new("project", project.to_string())]);
            }
            "PostToolUse" | "PostToolUseFailure" => {
                let obs_type = classify_tool(payload.tool_name.as_deref().unwrap_or(""));
                meter.u64_counter("nmem_observations_total").build().add(
                    1,
                    &[
                        KeyValue::new("obs_type", obs_type),
                        KeyValue::new("project", project.to_string()),
                        KeyValue::new(
                            "tool_name",
                            payload
                                .tool_name
                                .as_deref()
                                .unwrap_or("")
                                .to_string(),
                        ),
                    ],
                );
            }
            _ => {}
        }
    }

    meter
        .f64_histogram("nmem_record_duration_seconds")
        .build()
        .record(start.elapsed().as_secs_f64(), &[]);

    if let Err(e) = provider.shutdown() {
        eprintln!("nmem: metrics shutdown: {e}");
    }
}
