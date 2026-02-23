use crate::cli::MarkArgs;
use crate::db::open_db;
use crate::s1_record::stream_observation_to_logs;
use crate::s2_classify;
use crate::s2_locus;
use crate::s2_novelty;
use crate::s2_scope;
use crate::s5_config::{load_config, resolve_filter_params};
use crate::s5_filter::SecretFilter;
use crate::s5_project::derive_project;
use crate::NmemError;
use rusqlite::params;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn handle_mark(db_path: &Path, args: &MarkArgs) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;
    let config = load_config().unwrap_or_default();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // Derive project
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let project = args.project.clone().unwrap_or_else(|| derive_project(&cwd));

    // Filter secrets
    let filter_params = resolve_filter_params(&config, Some(&project));
    let filter = SecretFilter::with_params(filter_params);
    let (filtered_text, redacted) = filter.redact(&args.text);

    if redacted {
        eprintln!("nmem: redacted potential secret from marker");
    }

    // Find most recent session for this project, or create one
    let session_id: String = conn
        .query_row(
            "SELECT s.id FROM sessions s WHERE s.project = ?1 ORDER BY s.started_at DESC LIMIT 1",
            params![project],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| format!("mark-{ts}"));

    let tx = conn.unchecked_transaction()?;

    // Ensure session exists (creates if mark-{ts} was generated)
    let session_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
        params![session_id],
        |r| r.get(0),
    )?;
    if !session_exists {
        tx.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES (?1, ?2, ?3)",
            params![session_id, project, ts],
        )?;
    }

    // Get current prompt_id for the session
    let prompt_id: Option<i64> = tx
        .query_row(
            "SELECT id FROM prompts WHERE session_id = ?1 ORDER BY timestamp DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .ok();

    // Classify all 5 dimensions
    let phase_result = s2_classify::classify(&filtered_text);
    let phase = phase_result.as_ref().map(|p| p.label);
    let classifier_run_id = phase_result.as_ref().and_then(|p| {
        s2_classify::ensure_classifier_run(&tx, "think-act", p.model_hash, None, None, None).ok()
    });

    let scope_result = s2_scope::classify_scope(&filtered_text);
    let scope = scope_result.as_ref().map(|s| s.label);
    let scope_run_id = scope_result.as_ref().and_then(|s| {
        s2_classify::ensure_classifier_run(&tx, "converge-diverge", s.model_hash, None, None, None)
            .ok()
    });

    let locus_result = s2_locus::classify_locus(&filtered_text);
    let locus = locus_result.as_ref().map(|r| r.label);
    let locus_run_id = locus_result.as_ref().and_then(|r| {
        s2_classify::ensure_classifier_run(&tx, "internal-external", r.model_hash, None, None, None)
            .ok()
    });

    let novelty_result = s2_novelty::classify_novelty(&filtered_text);
    let novelty = novelty_result.as_ref().map(|r| r.label);
    let novelty_run_id = novelty_result.as_ref().and_then(|r| {
        s2_classify::ensure_classifier_run(&tx, "routine-novel", r.model_hash, None, None, None)
            .ok()
    });

    // Friction is now computed at episode level (S4), not per-observation
    let friction: Option<&str> = None;
    let friction_run_id: Option<i64> = None;

    // Insert observation
    tx.execute(
        "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content, phase, classifier_run_id, scope, scope_run_id, locus, locus_run_id, novelty, novelty_run_id, friction, friction_run_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            session_id,
            prompt_id,
            ts,
            "marker",
            "AgentMarker",
            filtered_text,
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

    let obs_id = tx.last_insert_rowid();
    tx.commit()?;

    // Stream to VictoriaLogs — non-fatal
    stream_observation_to_logs(
        &session_id,
        &project,
        "marker",
        "",
        None,
        &filtered_text,
        phase,
        scope,
        locus,
        novelty,
        friction,
        &None,
    );

    println!("{obs_id}");
    Ok(())
}
