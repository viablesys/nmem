use crate::s3_learn::{intent_keywords, jaccard};
use crate::s5_config::SummarizationConfig;
use crate::NmemError;
use rusqlite::{params, Connection};

/// Jaccard threshold for intra-session episode boundaries.
/// Lower than s3_learn's 0.4 because intra-session prompts are shorter
/// and more varied. The keyword bag grows as prompts accumulate, so it
/// takes a genuine intent shift (Bayesian surprise) to drop below this.
const BOUNDARY_THRESHOLD: f64 = 0.15;

/// Minimum word count for a prompt to be considered substantive.
/// Prompts shorter than this are treated as continuations ("yes", "ok", "do it").
const MIN_WORDS: usize = 5;

/// An episode detected from user prompt intent analysis.
pub struct Episode {
    pub session_id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub intent: String,
    pub first_prompt_id: i64,
    pub last_prompt_id: i64,
    pub keywords: Vec<String>,
}

/// A fully annotated episode ready for storage.
struct WorkUnitRow {
    session_id: String,
    started_at: i64,
    ended_at: Option<i64>,
    intent: String,
    first_prompt_id: i64,
    last_prompt_id: i64,
    hot_files: String,
    phase_signature: String,
    obs_count: i64,
    obs_trace: Option<String>,
}

/// Ensure the user_intent_stream view exists (idempotent).
fn ensure_view(conn: &Connection) -> Result<(), NmemError> {
    conn.execute_batch(
        "CREATE VIEW IF NOT EXISTS user_intent_stream AS
         SELECT
             p.id as prompt_id,
             p.session_id,
             p.timestamp,
             p.content,
             LENGTH(p.content) - LENGTH(REPLACE(p.content, ' ', '')) + 1 as word_count
         FROM prompts p
         WHERE p.source = 'user'",
    )?;
    Ok(())
}

/// Detect episode boundaries from user prompts in a session.
pub fn detect_episodes(conn: &Connection, session_id: &str) -> Result<Vec<Episode>, NmemError> {
    ensure_view(conn)?;

    let mut stmt = conn.prepare(
        "SELECT prompt_id, timestamp, content, word_count
         FROM user_intent_stream
         WHERE session_id = ?1
         ORDER BY prompt_id ASC",
    )?;

    struct PromptRow {
        id: i64,
        timestamp: i64,
        content: String,
        word_count: i64,
    }

    let rows: Vec<PromptRow> = stmt
        .query_map(params![session_id], |r| {
            Ok(PromptRow {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                content: r.get(2)?,
                word_count: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut episodes: Vec<Episode> = Vec::new();
    let mut current_keywords: Vec<String> = Vec::new();
    let mut current_episode: Option<Episode> = None;

    for row in &rows {
        if row.word_count < MIN_WORDS as i64 {
            // Terse prompt — continuation of current episode
            if let Some(ep) = current_episode.as_mut() {
                ep.last_prompt_id = row.id;
                ep.ended_at = Some(row.timestamp);
            } else {
                // Session starts with a terse prompt — start an episode anyway
                current_keywords.clear();
                current_episode = Some(Episode {
                    session_id: session_id.to_string(),
                    started_at: row.timestamp,
                    ended_at: Some(row.timestamp),
                    intent: row.content.clone(),
                    first_prompt_id: row.id,
                    last_prompt_id: row.id,
                    keywords: Vec::new(),
                });
            }
            continue;
        }

        let new_keywords = intent_keywords(&row.content);

        if current_episode.is_none() {
            // First substantive prompt — start first episode
            current_keywords = new_keywords;
            current_episode = Some(Episode {
                session_id: session_id.to_string(),
                started_at: row.timestamp,
                ended_at: Some(row.timestamp),
                intent: row.content.clone(),
                first_prompt_id: row.id,
                last_prompt_id: row.id,
                keywords: current_keywords.clone(),
            });
            continue;
        }

        let similarity = jaccard(&new_keywords, &current_keywords);

        if similarity < BOUNDARY_THRESHOLD {
            // Intent shift — close current episode, start new one
            if let Some(ep) = current_episode.take() {
                episodes.push(ep);
            }
            current_keywords = new_keywords;
            current_episode = Some(Episode {
                session_id: session_id.to_string(),
                started_at: row.timestamp,
                ended_at: Some(row.timestamp),
                intent: row.content.clone(),
                first_prompt_id: row.id,
                last_prompt_id: row.id,
                keywords: current_keywords.clone(),
            });
        } else {
            // Same episode — merge keywords, extend
            for kw in &new_keywords {
                if !current_keywords.contains(kw) {
                    current_keywords.push(kw.clone());
                }
            }
            if let Some(ep) = current_episode.as_mut() {
                ep.last_prompt_id = row.id;
                ep.ended_at = Some(row.timestamp);
                ep.keywords = current_keywords.clone();
            }
        }
    }

    // Close final episode
    if let Some(ep) = current_episode.take() {
        episodes.push(ep);
    }

    Ok(episodes)
}

/// Annotate an episode with observation metadata from the DB.
fn annotate_episode(conn: &Connection, episode: &Episode) -> Result<WorkUnitRow, NmemError> {
    // Hot files: distinct file_paths for observations in this episode's prompt range
    let hot_files: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT file_path FROM observations
             WHERE session_id = ?1
               AND prompt_id >= ?2 AND prompt_id <= ?3
               AND file_path IS NOT NULL
             ORDER BY file_path",
        )?;
        stmt.query_map(
            params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
            |r| r.get(0),
        )?
        .collect::<Result<_, _>>()?
    };

    // Phase signature: use classifier labels (think/act) when available,
    // fall back to obs_type heuristic for old unclassified observations.
    // Also aggregate scope (converge/diverge) from classifier labels.
    let (investigate, execute, failures, diverge, converge, internal, external, routine, novel, smooth, friction) = {
        let mut stmt = conn.prepare(
            "SELECT phase, obs_type, scope, locus, novelty, friction, COUNT(*) FROM observations
             WHERE session_id = ?1
               AND prompt_id >= ?2 AND prompt_id <= ?3
             GROUP BY phase, obs_type, scope, locus, novelty, friction",
        )?;
        let mut inv = 0i64;
        let mut exe = 0i64;
        let mut div = 0i64;
        let mut conv = 0i64;
        let mut int = 0i64;
        let mut ext = 0i64;
        let mut rout = 0i64;
        let mut nov = 0i64;
        let mut sm = 0i64;
        let mut fric = 0i64;

        let mut rows = stmt.query(params![
            episode.session_id,
            episode.first_prompt_id,
            episode.last_prompt_id,
        ])?;
        while let Some(row) = rows.next()? {
            let phase: Option<String> = row.get(0)?;
            let obs_type: String = row.get(1)?;
            let scope: Option<String> = row.get(2)?;
            let locus: Option<String> = row.get(3)?;
            let novelty: Option<String> = row.get(4)?;
            let friction: Option<String> = row.get(5)?;
            let count: i64 = row.get(6)?;
            match phase.as_deref() {
                Some("think") => inv += count,
                Some("act") => exe += count,
                None => match obs_type.as_str() {
                    "file_read" | "search" | "web_search" | "web_fetch" => inv += count,
                    "file_edit" | "file_write" | "git_commit" | "git_push" | "command" => exe += count,
                    _ => {}
                },
                _ => {}
            }
            match scope.as_deref() {
                Some("diverge") => div += count,
                Some("converge") => conv += count,
                _ => {}
            }
            match locus.as_deref() {
                Some("internal") => int += count,
                Some("external") => ext += count,
                _ => {}
            }
            match novelty.as_deref() {
                Some("routine") => rout += count,
                Some("novel") => nov += count,
                _ => {}
            }
            match friction.as_deref() {
                Some("smooth") => sm += count,
                Some("friction") => fric += count,
                _ => {}
            }
        }

        // Count failures separately from metadata
        let fail: i64 = conn.query_row(
            "SELECT COUNT(*) FROM observations
             WHERE session_id = ?1
               AND prompt_id >= ?2 AND prompt_id <= ?3
               AND json_extract(metadata, '$.failed') = 1",
            params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
            |r| r.get(0),
        )?;

        (inv, exe, fail, div, conv, int, ext, rout, nov, sm, fric)
    };

    let obs_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM observations
         WHERE session_id = ?1
           AND prompt_id >= ?2 AND prompt_id <= ?3",
        params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
        |r| r.get(0),
    )?;

    let hot_files_json = serde_json::to_string(&hot_files)?;
    let phase_sig = serde_json::json!({
        "investigate": investigate,
        "execute": execute,
        "failures": failures,
        "diverge": diverge,
        "converge": converge,
        "internal": internal,
        "external": external,
        "routine": routine,
        "novel": novel,
        "smooth": smooth,
        "friction": friction,
    })
    .to_string();

    // Compute obs_trace: compact per-observation fingerprint for S3 sweep safety
    let obs_trace = {
        let mut stmt = conn.prepare(
            "SELECT timestamp, obs_type, file_path, phase, scope, locus, novelty, friction,
                    CASE WHEN json_extract(metadata, '$.failed') = 1 THEN 1 ELSE 0 END as failed
             FROM observations
             WHERE session_id = ?1 AND prompt_id >= ?2 AND prompt_id <= ?3
             ORDER BY timestamp ASC",
        )?;
        let trace: Vec<serde_json::Value> = stmt
            .query_map(
                params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
                |r| {
                    let ts: i64 = r.get(0)?;
                    let obs_type: String = r.get(1)?;
                    let file_path: Option<String> = r.get(2)?;
                    let phase: Option<String> = r.get(3)?;
                    let scope: Option<String> = r.get(4)?;
                    let locus: Option<String> = r.get(5)?;
                    let novelty: Option<String> = r.get(6)?;
                    let friction: Option<String> = r.get(7)?;
                    let failed: bool = r.get::<_, i64>(8)? != 0;

                    let mut obj = serde_json::Map::new();
                    obj.insert("t".into(), serde_json::Value::Number(ts.into()));
                    obj.insert("type".into(), serde_json::Value::String(obs_type));
                    if let Some(fp) = file_path {
                        obj.insert("fp".into(), serde_json::Value::String(fp));
                    }
                    if let Some(p) = phase {
                        obj.insert("p".into(), serde_json::Value::String(p));
                    }
                    if let Some(s) = scope {
                        obj.insert("s".into(), serde_json::Value::String(s));
                    }
                    if let Some(l) = locus {
                        obj.insert("l".into(), serde_json::Value::String(l));
                    }
                    if let Some(n) = novelty {
                        obj.insert("n".into(), serde_json::Value::String(n));
                    }
                    if let Some(f) = friction {
                        obj.insert("f".into(), serde_json::Value::String(f));
                    }
                    if failed {
                        obj.insert("fail".into(), serde_json::Value::Bool(true));
                    }
                    Ok(serde_json::Value::Object(obj))
                },
            )?
            .collect::<Result<_, _>>()?;

        if trace.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&trace)?)
        }
    };

    Ok(WorkUnitRow {
        session_id: episode.session_id.clone(),
        started_at: episode.started_at,
        ended_at: episode.ended_at,
        intent: episode.intent.clone(),
        first_prompt_id: episode.first_prompt_id,
        last_prompt_id: episode.last_prompt_id,
        hot_files: hot_files_json,
        phase_signature: phase_sig,
        obs_count,
        obs_trace,
    })
}

/// Store annotated episodes in the work_units table.
fn store_episodes(conn: &Connection, episodes: &[WorkUnitRow]) -> Result<(), NmemError> {
    let mut stmt = conn.prepare(
        "INSERT INTO work_units (session_id, started_at, ended_at, intent,
         first_prompt_id, last_prompt_id, hot_files, phase_signature, obs_count, obs_trace)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;

    for ep in episodes {
        stmt.execute(params![
            ep.session_id,
            ep.started_at,
            ep.ended_at,
            ep.intent,
            ep.first_prompt_id,
            ep.last_prompt_id,
            ep.hot_files,
            ep.phase_signature,
            ep.obs_count,
            ep.obs_trace,
        ])?;
    }

    Ok(())
}

const EPISODE_SYSTEM_PROMPT: &str =
    "You produce structured JSON summaries of work episodes within a coding session. The consumer is an AI agent's cross-session memory. Optimize for context reconstruction. Return ONLY valid JSON, no markdown, no explanation.";

const EPISODE_USER_TEMPLATE: &str = r#"Summarize this work episode (one coherent chunk of work within a larger session) for the next AI agent session.

Return JSON with these fields:
- "intent": What was being accomplished in this specific episode
- "learned": Decisions made, constraints discovered, conclusions reached
- "completed": What was done in this episode
- "notes": Errors encountered, failed approaches, deviations from expected patterns

Episode context:
- Hot files: {HOT_FILES}
- Phase character: {PHASE_SIG}
- Observation count: {OBS_COUNT}

Prompts and actions in this episode:
{PAYLOAD}

Return ONLY the JSON object."#;

/// Gather episode-scoped payload for narrative generation.
fn gather_episode_payload(
    conn: &Connection,
    episode: &WorkUnitRow,
) -> Result<Option<String>, NmemError> {
    // Count user prompts in range
    let prompt_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM prompts
         WHERE session_id = ?1 AND source = 'user'
           AND id >= ?2 AND id <= ?3",
        params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
        |r| r.get(0),
    )?;

    // Skip single-prompt episodes (the prompt IS the story)
    if prompt_count < 2 {
        return Ok(None);
    }

    // Skip sparse episodes
    if episode.obs_count < 3 {
        return Ok(None);
    }

    let mut out = String::new();

    // User prompts in range
    let mut prompt_stmt = conn.prepare(
        "SELECT content FROM prompts
         WHERE session_id = ?1 AND source = 'user'
           AND id >= ?2 AND id <= ?3
         ORDER BY id ASC LIMIT 10",
    )?;
    let prompts: Vec<String> = prompt_stmt
        .query_map(
            params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
            |r| r.get(0),
        )?
        .collect::<Result<_, _>>()?;

    if !prompts.is_empty() {
        out.push_str("User prompts:\n");
        for p in &prompts {
            let truncated: String = p.chars().take(100).collect();
            out.push_str(&format!("- {truncated}\n"));
        }
        out.push('\n');
    }

    // Agent reasoning in range
    let mut thinking_stmt = conn.prepare(
        "SELECT content FROM prompts
         WHERE session_id = ?1 AND source = 'agent'
           AND id >= ?2 AND id <= ?3
         ORDER BY id ASC LIMIT 5",
    )?;
    let thinking: Vec<String> = thinking_stmt
        .query_map(
            params![episode.session_id, episode.first_prompt_id, episode.last_prompt_id],
            |r| r.get(0),
        )?
        .collect::<Result<_, _>>()?;

    if !thinking.is_empty() {
        out.push_str("Agent reasoning:\n");
        for t in &thinking {
            let truncated: String = t.chars().take(200).collect();
            out.push_str(&format!("- {truncated}\n"));
        }
        out.push('\n');
    }

    // Observations in range — include classifier labels and failure metadata
    let mut obs_stmt = conn.prepare(
        "SELECT obs_type, file_path, content, phase, scope, locus, novelty, metadata
         FROM observations
         WHERE session_id = ?1
           AND prompt_id >= ?2 AND prompt_id <= ?3
         ORDER BY timestamp ASC LIMIT 30",
    )?;

    out.push_str("Actions:\n");
    let mut rows = obs_stmt.query(params![
        episode.session_id,
        episode.first_prompt_id,
        episode.last_prompt_id,
    ])?;
    while let Some(row) = rows.next()? {
        let obs_type: String = row.get(0)?;
        let file_path: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let phase: Option<String> = row.get(3)?;
        let scope: Option<String> = row.get(4)?;
        let locus: Option<String> = row.get(5)?;
        let novelty: Option<String> = row.get(6)?;
        let metadata_str: Option<String> = row.get(7)?;

        let display = crate::s1_4_summarize::format_action_line(
            &obs_type, file_path.as_deref(), &content,
            phase.as_deref(), scope.as_deref(), locus.as_deref(), novelty.as_deref(),
            metadata_str.as_deref(),
        );
        out.push_str(&format!("{display}\n"));
    }

    Ok(Some(out))
}

/// Generate narrative for a single episode via LLM.
fn generate_narrative(
    conn: &Connection,
    episode: &WorkUnitRow,
    config: &SummarizationConfig,
) -> Result<Option<String>, NmemError> {
    let payload = match gather_episode_payload(conn, episode)? {
        Some(p) => p,
        None => return Ok(None),
    };

    let user_content = EPISODE_USER_TEMPLATE
        .replace("{HOT_FILES}", &episode.hot_files)
        .replace("{PHASE_SIG}", &episode.phase_signature)
        .replace("{OBS_COUNT}", &episode.obs_count.to_string())
        .replace("{PAYLOAD}", &payload);

    let body = serde_json::json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": EPISODE_SYSTEM_PROMPT},
            {"role": "user", "content": user_content},
        ],
        "temperature": 0.0,
        "max_tokens": 512,
    });

    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(config.timeout_secs)))
            .build(),
    );

    let result = agent
        .post(&config.endpoint)
        .send_json(&body)
        .map_err(|e| NmemError::Config(format!("episode narrative request: {e}")));

    let resp: serde_json::Value = match result {
        Ok(mut r) => r
            .body_mut()
            .read_json()
            .map_err(|e| NmemError::Config(format!("episode narrative response: {e}")))?,
        Err(e) => {
            // Try fallback if available
            if let Some(fallback) = &config.fallback_endpoint {
                let mut r = agent
                    .post(fallback)
                    .send_json(&body)
                    .map_err(|e| NmemError::Config(format!("episode narrative fallback: {e}")))?;
                r.body_mut()
                    .read_json()
                    .map_err(|e| NmemError::Config(format!("episode narrative fallback response: {e}")))?
            } else {
                return Err(e);
            }
        }
    };

    let text = resp
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| NmemError::Config("no content in episode narrative response".into()))?;

    Ok(Some(text.to_string()))
}

/// Update a work_unit row with narrative summary.
fn store_narrative(conn: &Connection, session_id: &str, first_prompt_id: i64, narrative: &str) -> Result<(), NmemError> {
    conn.execute(
        "UPDATE work_units SET summary = ?1 WHERE session_id = ?2 AND first_prompt_id = ?3",
        params![narrative, session_id, first_prompt_id],
    )?;
    Ok(())
}

/// Apply episode-level friction labels to observations within a session's episodes.
/// An episode has friction if it contains any failures (metadata.failed = true).
/// All observations in that episode inherit the label. Observations not in any
/// episode get NULL friction (unknown without episode context).
fn apply_episode_friction(conn: &Connection, session_id: &str) -> Result<(), NmemError> {
    let mut stmt = conn.prepare(
        "SELECT first_prompt_id, last_prompt_id, phase_signature
         FROM work_units WHERE session_id = ?1",
    )?;

    let units: Vec<(i64, i64, String)> = stmt
        .query_map(params![session_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<_, _>>()?;

    for (first, last, sig_json) in &units {
        let label = friction_label_from_signature(sig_json);
        conn.execute(
            "UPDATE observations SET friction = ?1, friction_run_id = NULL
             WHERE session_id = ?2 AND prompt_id >= ?3 AND prompt_id <= ?4",
            params![label, session_id, first, last],
        )?;
    }

    Ok(())
}

/// Determine friction label from a phase_signature JSON string.
/// Returns "friction" if failures > 0, "smooth" otherwise.
fn friction_label_from_signature(sig_json: &str) -> &'static str {
    let failures = serde_json::from_str::<serde_json::Value>(sig_json)
        .ok()
        .and_then(|v| v.get("failures")?.as_i64())
        .unwrap_or(0);
    if failures > 0 { "friction" } else { "smooth" }
}

/// Backfill friction labels for all historical episodes.
/// Walks all work_units, applies the heuristic (failures > 0 → friction),
/// and updates observations in each episode's prompt range.
pub fn backfill_episode_friction(db_path: &std::path::Path) -> Result<(), NmemError> {
    let conn = crate::db::open_db(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT session_id, first_prompt_id, last_prompt_id, phase_signature
         FROM work_units ORDER BY session_id, first_prompt_id",
    )?;

    let units: Vec<(String, i64, i64, String)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<Result<_, _>>()?;

    let mut updated = 0u64;
    for (session_id, first, last, sig_json) in &units {
        let label = friction_label_from_signature(sig_json);
        let n = conn.execute(
            "UPDATE observations SET friction = ?1, friction_run_id = NULL
             WHERE session_id = ?2 AND prompt_id >= ?3 AND prompt_id <= ?4",
            params![label, session_id, first, last],
        )?;
        updated += n as u64;
    }

    // Clear friction on orphan observations (not in any episode)
    let orphaned = conn.execute(
        "UPDATE observations SET friction = NULL, friction_run_id = NULL
         WHERE friction IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM work_units wu
               WHERE wu.session_id = observations.session_id
                 AND observations.prompt_id >= wu.first_prompt_id
                 AND observations.prompt_id <= wu.last_prompt_id
           )",
        [],
    )?;

    eprintln!(
        "nmem: friction backfill complete — {} episodes, {} observations updated, {} orphans cleared",
        units.len(),
        updated,
        orphaned,
    );

    Ok(())
}

/// Backfill obs_trace for existing episodes that don't have one.
pub fn backfill_obs_trace(db_path: &std::path::Path) -> Result<(), NmemError> {
    let conn = crate::db::open_db(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT id, session_id, first_prompt_id, last_prompt_id
         FROM work_units WHERE obs_trace IS NULL
         ORDER BY session_id, first_prompt_id",
    )?;

    let units: Vec<(i64, String, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<Result<_, _>>()?;

    let mut filled = 0u64;
    let mut trace_stmt = conn.prepare(
        "SELECT timestamp, obs_type, file_path, phase, scope, locus, novelty, friction,
                CASE WHEN json_extract(metadata, '$.failed') = 1 THEN 1 ELSE 0 END as failed
         FROM observations
         WHERE session_id = ?1 AND prompt_id >= ?2 AND prompt_id <= ?3
         ORDER BY timestamp ASC",
    )?;

    for (wu_id, session_id, first, last) in &units {
        let trace: Vec<serde_json::Value> = trace_stmt
            .query_map(params![session_id, first, last], |r| {
                let ts: i64 = r.get(0)?;
                let obs_type: String = r.get(1)?;
                let file_path: Option<String> = r.get(2)?;
                let phase: Option<String> = r.get(3)?;
                let scope: Option<String> = r.get(4)?;
                let locus: Option<String> = r.get(5)?;
                let novelty: Option<String> = r.get(6)?;
                let friction: Option<String> = r.get(7)?;
                let failed: bool = r.get::<_, i64>(8)? != 0;

                let mut obj = serde_json::Map::new();
                obj.insert("t".into(), serde_json::Value::Number(ts.into()));
                obj.insert("type".into(), serde_json::Value::String(obs_type));
                if let Some(fp) = file_path {
                    obj.insert("fp".into(), serde_json::Value::String(fp));
                }
                if let Some(p) = phase {
                    obj.insert("p".into(), serde_json::Value::String(p));
                }
                if let Some(s) = scope {
                    obj.insert("s".into(), serde_json::Value::String(s));
                }
                if let Some(l) = locus {
                    obj.insert("l".into(), serde_json::Value::String(l));
                }
                if let Some(n) = novelty {
                    obj.insert("n".into(), serde_json::Value::String(n));
                }
                if let Some(f) = friction {
                    obj.insert("f".into(), serde_json::Value::String(f));
                }
                if failed {
                    obj.insert("fail".into(), serde_json::Value::Bool(true));
                }
                Ok(serde_json::Value::Object(obj))
            })?
            .collect::<Result<_, _>>()?;

        if !trace.is_empty() {
            let json = serde_json::to_string(&trace)?;
            conn.execute(
                "UPDATE work_units SET obs_trace = ?1 WHERE id = ?2",
                params![json, wu_id],
            )?;
            filled += 1;
        }
    }

    eprintln!(
        "nmem: obs_trace backfill complete — {} of {} episodes filled",
        filled,
        units.len(),
    );

    Ok(())
}

/// Orchestrator: detect episodes, annotate, and store. No narrative generation.
/// Idempotent: skips if work_units already exist for this session.
pub fn detect_and_store_episodes(
    conn: &Connection,
    session_id: &str,
) -> Result<usize, NmemError> {
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM work_units WHERE session_id = ?1",
        params![session_id],
        |r| r.get(0),
    )?;
    if existing > 0 {
        return Ok(0);
    }

    let episodes = detect_episodes(conn, session_id)?;
    if episodes.is_empty() {
        return Ok(0);
    }

    let mut annotated = Vec::with_capacity(episodes.len());
    for ep in &episodes {
        annotated.push(annotate_episode(conn, ep)?);
    }

    store_episodes(conn, &annotated)?;
    apply_episode_friction(conn, session_id)?;
    Ok(annotated.len())
}

/// Full pipeline: detect, annotate, store, and generate narratives.
/// Idempotent: skips if work_units already exist for this session.
pub fn detect_and_narrate_episodes(
    conn: &Connection,
    session_id: &str,
    config: &SummarizationConfig,
) -> Result<usize, NmemError> {
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM work_units WHERE session_id = ?1",
        params![session_id],
        |r| r.get(0),
    )?;
    if existing > 0 {
        return Ok(0);
    }

    let episodes = detect_episodes(conn, session_id)?;
    if episodes.is_empty() {
        return Ok(0);
    }

    let mut annotated = Vec::with_capacity(episodes.len());
    for ep in &episodes {
        annotated.push(annotate_episode(conn, ep)?);
    }

    store_episodes(conn, &annotated)?;
    apply_episode_friction(conn, session_id)?;
    let count = annotated.len();

    // Generate narratives if summarization is enabled
    if config.enabled {
        for ep in &annotated {
            match generate_narrative(conn, ep, config) {
                Ok(Some(narrative)) => {
                    if let Err(e) = store_narrative(conn, &ep.session_id, ep.first_prompt_id, &narrative) {
                        eprintln!("nmem: episode narrative store failed: {e}");
                    }
                }
                Ok(None) => {} // Skipped (too sparse)
                Err(e) => eprintln!("nmem: episode narrative failed (non-fatal): {e}"),
            }
        }
    }

    Ok(count)
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

    fn insert_prompt(conn: &Connection, session_id: &str, ts: i64, content: &str) -> i64 {
        conn.execute(
            "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?1, ?2, 'user', ?3)",
            params![session_id, ts, content],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_obs_with_prompt(
        conn: &Connection,
        session_id: &str,
        prompt_id: i64,
        ts: i64,
        obs_type: &str,
        file_path: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content)
             VALUES (?1, ?2, ?3, ?4, 'PostToolUse', 'content')",
            params![session_id, prompt_id, ts, obs_type],
        )
        .unwrap();
        if let Some(fp) = file_path {
            let id = conn.last_insert_rowid();
            conn.execute(
                "UPDATE observations SET file_path = ?1 WHERE id = ?2",
                params![fp, id],
            )
            .unwrap();
        }
    }

    #[test]
    fn single_topic_produces_one_episode() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        insert_prompt(&conn, "s1", 1010, "update the authentication test for the login fix");
        insert_prompt(&conn, "s1", 1020, "commit the authentication login handler fix");

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert_eq!(episodes.len(), 1, "same-topic prompts should be 1 episode");
        assert!(episodes[0].intent.contains("authentication"));
    }

    #[test]
    fn clear_intent_shift_produces_two_episodes() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        insert_prompt(&conn, "s1", 1010, "update the authentication test for the login fix");
        insert_prompt(&conn, "s1", 1020, "now refactor the database schema migration system");
        insert_prompt(&conn, "s1", 1030, "add a new migration for the users table schema");

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert_eq!(episodes.len(), 2, "clear intent shift should produce 2 episodes");
        assert!(episodes[0].intent.contains("authentication"));
        assert!(episodes[1].intent.contains("refactor") || episodes[1].intent.contains("database"));
    }

    #[test]
    fn terse_prompts_are_continuations() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        insert_prompt(&conn, "s1", 1010, "yes");
        insert_prompt(&conn, "s1", 1020, "ok do it");
        insert_prompt(&conn, "s1", 1030, "looks good");

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert_eq!(episodes.len(), 1, "terse prompts should not create new episodes");
    }

    #[test]
    fn single_prompt_session_produces_one_episode() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        insert_prompt(&conn, "s1", 1000, "implement the new feature for user notifications");

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert_eq!(episodes.len(), 1);
    }

    #[test]
    fn empty_session_produces_no_episodes() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert!(episodes.is_empty());
    }

    #[test]
    fn annotate_computes_hot_files_and_phase() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        let p2 = insert_prompt(&conn, "s1", 1010, "update the authentication test for login");

        // Some observations in the prompt range
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1002, "file_read", Some("/src/handler.rs"));
        insert_obs_with_prompt(&conn, "s1", p2, 1011, "file_edit", Some("/src/auth.rs"));
        insert_obs_with_prompt(&conn, "s1", p2, 1012, "file_edit", Some("/src/handler.rs"));

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert_eq!(episodes.len(), 1);

        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        assert_eq!(annotated.obs_count, 4);

        let hot_files: Vec<String> = serde_json::from_str(&annotated.hot_files).unwrap();
        assert!(hot_files.contains(&"/src/auth.rs".to_string()));
        assert!(hot_files.contains(&"/src/handler.rs".to_string()));

        let phase: serde_json::Value = serde_json::from_str(&annotated.phase_signature).unwrap();
        assert_eq!(phase["investigate"], 2); // 2 file_reads (NULL phase → obs_type fallback)
        assert_eq!(phase["execute"], 2); // 2 file_edits (NULL phase → obs_type fallback)
    }

    #[test]
    fn annotate_uses_classifier_phase_over_obs_type() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "investigate the auth module");

        // file_read with phase="think" — classifier agrees with obs_type
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"));
        // file_read with phase="act" — classifier overrides obs_type
        // (e.g., reading a file to verify a fix is act, not think)
        insert_obs_with_prompt(&conn, "s1", p1, 1002, "file_read", Some("/src/handler.rs"));
        conn.execute("UPDATE observations SET phase = 'act' WHERE timestamp = 1002", []).unwrap();
        // file_edit with phase="think" — classifier overrides obs_type
        // (e.g., adding a debug print to investigate)
        insert_obs_with_prompt(&conn, "s1", p1, 1003, "file_edit", Some("/src/auth.rs"));
        conn.execute("UPDATE observations SET phase = 'think' WHERE timestamp = 1003", []).unwrap();
        // command with no phase — falls back to obs_type (execute)
        insert_obs_with_prompt(&conn, "s1", p1, 1004, "command", None);

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();

        let phase: serde_json::Value = serde_json::from_str(&annotated.phase_signature).unwrap();
        // think: file_read(NULL→fallback) + file_edit(phase=think) = 2
        // act: file_read(phase=act) + command(NULL→fallback) = 2
        assert_eq!(phase["investigate"], 2);
        assert_eq!(phase["execute"], 2);
    }

    #[test]
    fn detect_and_store_roundtrip() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"));

        let p2 = insert_prompt(&conn, "s1", 1010, "now refactor the database schema migration system");
        insert_obs_with_prompt(&conn, "s1", p2, 1011, "file_edit", Some("/src/schema.rs"));

        let count = detect_and_store_episodes(&conn, "s1").unwrap();
        assert_eq!(count, 2);

        // Verify stored in work_units
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_units WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 2);

        // Verify intents stored
        let mut stmt = conn
            .prepare("SELECT intent FROM work_units WHERE session_id = 's1' ORDER BY started_at")
            .unwrap();
        let intents: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(intents[0].contains("authentication"));
        assert!(intents[1].contains("refactor") || intents[1].contains("database"));
    }

    #[test]
    fn schema_migration_applies() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();

        // Verify work_units table exists
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(tables.contains(&"work_units".into()));
    }

    #[test]
    fn narrative_skipped_for_single_prompt() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "implement the new feature for user notifications");
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/main.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1002, "file_edit", Some("/src/main.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1003, "command", None);

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        let payload = gather_episode_payload(&conn, &annotated).unwrap();
        assert!(payload.is_none(), "single-prompt episode should skip narrative");
    }

    #[test]
    fn narrative_skipped_for_sparse_episode() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "implement the new feature for user notifications");
        let _p2 = insert_prompt(&conn, "s1", 1010, "update the notification handler tests");
        // Only 2 observations — below threshold
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/main.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1002, "file_read", Some("/src/test.rs"));

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        let payload = gather_episode_payload(&conn, &annotated).unwrap();
        assert!(payload.is_none(), "sparse episode should skip narrative");
    }

    #[test]
    fn narrative_gathered_for_substantial_episode() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        let p2 = insert_prompt(&conn, "s1", 1010, "update the authentication test for the login fix");
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1002, "search", None);
        insert_obs_with_prompt(&conn, "s1", p2, 1011, "file_edit", Some("/src/auth.rs"));
        insert_obs_with_prompt(&conn, "s1", p2, 1012, "command", None);

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        let payload = gather_episode_payload(&conn, &annotated).unwrap();
        assert!(payload.is_some());
        let text = payload.unwrap();
        assert!(text.contains("authentication"));
        assert!(text.contains("[file_read]"));
    }

    /// Helper: insert observation with classifier labels and optional metadata.
    fn insert_obs_classified(
        conn: &Connection,
        session_id: &str,
        prompt_id: i64,
        ts: i64,
        obs_type: &str,
        file_path: Option<&str>,
        phase: Option<&str>,
        scope: Option<&str>,
        locus: Option<&str>,
        novelty: Option<&str>,
        metadata: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO observations (session_id, prompt_id, timestamp, obs_type, source_event, content, file_path, phase, scope, locus, novelty, metadata)
             VALUES (?1, ?2, ?3, ?4, 'PostToolUse', 'content', ?5, ?6, ?7, ?8, ?9, ?10)",
            params![session_id, prompt_id, ts, obs_type, file_path, phase, scope, locus, novelty, metadata],
        )
        .unwrap();
    }

    #[test]
    fn obs_trace_captures_all_fields() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");

        // Observation with all classifier labels
        insert_obs_classified(
            &conn, "s1", p1, 1001, "file_edit", Some("/src/auth.rs"),
            Some("act"), Some("converge"), Some("internal"), Some("routine"), None,
        );
        // Observation with failed metadata
        insert_obs_classified(
            &conn, "s1", p1, 1002, "command", None,
            Some("act"), Some("diverge"), Some("external"), Some("novel"),
            Some(r#"{"failed":true}"#),
        );
        // Observation with file_path but partial labels
        insert_obs_classified(
            &conn, "s1", p1, 1003, "file_read", Some("/src/handler.rs"),
            Some("think"), None, None, None, None,
        );

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        let trace_json = annotated.obs_trace.expect("obs_trace should be Some");
        let trace: Vec<serde_json::Value> = serde_json::from_str(&trace_json).unwrap();

        assert_eq!(trace.len(), 3);

        // First: all fields present
        let e0 = &trace[0];
        assert_eq!(e0["t"], 1001);
        assert_eq!(e0["type"], "file_edit");
        assert_eq!(e0["fp"], "/src/auth.rs");
        assert_eq!(e0["p"], "act");
        assert_eq!(e0["s"], "converge");
        assert_eq!(e0["l"], "internal");
        assert_eq!(e0["n"], "routine");
        assert!(e0.get("fail").is_none(), "non-failed should omit fail");

        // Second: failed flag present
        let e1 = &trace[1];
        assert_eq!(e1["t"], 1002);
        assert_eq!(e1["type"], "command");
        assert!(e1.get("fp").is_none(), "no file_path should omit fp");
        assert_eq!(e1["p"], "act");
        assert_eq!(e1["s"], "diverge");
        assert_eq!(e1["l"], "external");
        assert_eq!(e1["n"], "novel");
        assert_eq!(e1["fail"], true);

        // Third: partial labels — only p present
        let e2 = &trace[2];
        assert_eq!(e2["t"], 1003);
        assert_eq!(e2["type"], "file_read");
        assert_eq!(e2["fp"], "/src/handler.rs");
        assert_eq!(e2["p"], "think");
        assert!(e2.get("s").is_none(), "NULL scope should be absent");
        assert!(e2.get("l").is_none(), "NULL locus should be absent");
        assert!(e2.get("n").is_none(), "NULL novelty should be absent");
    }

    #[test]
    fn obs_trace_omits_null_optional_fields() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "implement the new feature for notifications");

        // Observations with NO classifier labels at all
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", None);
        insert_obs_with_prompt(&conn, "s1", p1, 1002, "command", None);

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        let trace_json = annotated.obs_trace.expect("obs_trace should be Some");
        let trace: Vec<serde_json::Value> = serde_json::from_str(&trace_json).unwrap();

        assert_eq!(trace.len(), 2);
        for entry in &trace {
            let obj = entry.as_object().unwrap();
            assert!(obj.contains_key("t"), "must have timestamp");
            assert!(obj.contains_key("type"), "must have obs_type");
            assert!(!obj.contains_key("p"), "NULL phase must be absent");
            assert!(!obj.contains_key("s"), "NULL scope must be absent");
            assert!(!obj.contains_key("l"), "NULL locus must be absent");
            assert!(!obj.contains_key("n"), "NULL novelty must be absent");
            assert!(!obj.contains_key("f"), "NULL friction must be absent");
            assert!(!obj.contains_key("fp"), "NULL file_path must be absent");
            assert!(!obj.contains_key("fail"), "non-failed must be absent");
        }
    }

    #[test]
    fn obs_trace_preserves_observation_order() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");

        // Insert out of timestamp order
        insert_obs_with_prompt(&conn, "s1", p1, 1005, "file_edit", Some("/src/c.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1001, "file_read", Some("/src/a.rs"));
        insert_obs_with_prompt(&conn, "s1", p1, 1003, "search", None);

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        let trace_json = annotated.obs_trace.expect("obs_trace should be Some");
        let trace: Vec<serde_json::Value> = serde_json::from_str(&trace_json).unwrap();

        assert_eq!(trace.len(), 3);
        let timestamps: Vec<i64> = trace.iter().map(|e| e["t"].as_i64().unwrap()).collect();
        assert_eq!(timestamps, vec![1001, 1003, 1005], "obs_trace must be sorted by timestamp ASC");
    }

    #[test]
    fn obs_trace_empty_episode_yields_none() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        // Create prompts but NO observations in their range
        let _p1 = insert_prompt(&conn, "s1", 1000, "investigate the logging framework options");

        let episodes = detect_episodes(&conn, "s1").unwrap();
        assert_eq!(episodes.len(), 1);

        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();
        assert!(annotated.obs_trace.is_none(), "episode with zero observations should have None obs_trace");
        assert_eq!(annotated.obs_count, 0);
    }

    #[test]
    fn obs_trace_survives_sweep() {
        use crate::s3_sweep::run_sweep;
        use crate::s5_config::RetentionConfig;
        use std::collections::HashMap;

        let conn = setup_db();
        insert_session(&conn, "s1");
        // Sweep requires session to be summarized
        conn.execute("UPDATE sessions SET summary = '{}' WHERE id = 's1'", []).unwrap();

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        insert_obs_classified(
            &conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"),
            Some("think"), Some("diverge"), None, None, None,
        );
        insert_obs_classified(
            &conn, "s1", p1, 1002, "file_edit", Some("/src/auth.rs"),
            Some("act"), Some("converge"), None, None, None,
        );

        // Run full detect_and_store to create work_units with obs_trace
        let count = detect_and_store_episodes(&conn, "s1").unwrap();
        assert_eq!(count, 1);

        // Verify obs_trace was stored
        let trace_before: Option<String> = conn
            .query_row("SELECT obs_trace FROM work_units WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert!(trace_before.is_some(), "obs_trace should exist before sweep");
        let trace_content = trace_before.unwrap();

        // Verify observations exist before sweep
        let obs_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(obs_before, 2);

        // Aggressive sweep: 0 days retention for all types
        let config = RetentionConfig {
            enabled: true,
            days: HashMap::from([
                ("file_read".into(), 0),
                ("file_edit".into(), 0),
            ]),
            max_db_size_mb: None,
        };
        let result = run_sweep(&conn, &config).unwrap();
        assert_eq!(result.deleted, 2, "sweep should delete both observations");

        // Observations gone
        let obs_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(obs_after, 0, "observations should be deleted by sweep");

        // obs_trace survives intact
        let trace_after: Option<String> = conn
            .query_row("SELECT obs_trace FROM work_units WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(trace_after.unwrap(), trace_content, "obs_trace must survive sweep unchanged");
    }

    #[test]
    fn obs_trace_backfill_fills_missing() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");
        insert_obs_classified(
            &conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"),
            Some("think"), Some("diverge"), None, None, None,
        );
        insert_obs_classified(
            &conn, "s1", p1, 1002, "file_edit", Some("/src/auth.rs"),
            Some("act"), Some("converge"), None, None, None,
        );

        // Manually insert work_unit with NULL obs_trace (simulating pre-obs_trace data)
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, ended_at, intent, first_prompt_id, last_prompt_id, hot_files, phase_signature, obs_count, obs_trace)
             VALUES ('s1', 1000, 1002, 'fix auth', ?1, ?2, '[]', '{}', 2, NULL)",
            params![p1, p1],
        ).unwrap();

        // Also insert a work_unit that already has obs_trace (should be untouched)
        let existing_trace = r#"[{"t":9999,"type":"command"}]"#;
        conn.execute(
            "INSERT INTO work_units (session_id, started_at, ended_at, intent, first_prompt_id, last_prompt_id, hot_files, phase_signature, obs_count, obs_trace)
             VALUES ('s1', 2000, 2001, 'other work', 999, 999, '[]', '{}', 1, ?1)",
            params![existing_trace],
        ).unwrap();

        // backfill_obs_trace needs a file-based DB, so we replicate the core logic inline
        // (the function uses open_db which manages encryption; in tests we use in-memory)
        {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, first_prompt_id, last_prompt_id
                 FROM work_units WHERE obs_trace IS NULL
                 ORDER BY session_id, first_prompt_id",
            ).unwrap();

            let units: Vec<(i64, String, i64, i64)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();

            assert_eq!(units.len(), 1, "only NULL obs_trace units should be selected");

            for (wu_id, session_id, first, last) in &units {
                let mut trace_stmt = conn.prepare(
                    "SELECT timestamp, obs_type, file_path, phase, scope, locus, novelty, friction,
                            CASE WHEN json_extract(metadata, '$.failed') = 1 THEN 1 ELSE 0 END as failed
                     FROM observations
                     WHERE session_id = ?1 AND prompt_id >= ?2 AND prompt_id <= ?3
                     ORDER BY timestamp ASC",
                ).unwrap();
                let trace: Vec<serde_json::Value> = trace_stmt
                    .query_map(params![session_id, first, last], |r| {
                        let mut obj = serde_json::Map::new();
                        obj.insert("t".into(), serde_json::Value::Number(r.get::<_, i64>(0)?.into()));
                        obj.insert("type".into(), serde_json::Value::String(r.get(1)?));
                        if let Some(fp) = r.get::<_, Option<String>>(2)? {
                            obj.insert("fp".into(), serde_json::Value::String(fp));
                        }
                        if let Some(p) = r.get::<_, Option<String>>(3)? {
                            obj.insert("p".into(), serde_json::Value::String(p));
                        }
                        if let Some(s) = r.get::<_, Option<String>>(4)? {
                            obj.insert("s".into(), serde_json::Value::String(s));
                        }
                        if let Some(l) = r.get::<_, Option<String>>(5)? {
                            obj.insert("l".into(), serde_json::Value::String(l));
                        }
                        if let Some(n) = r.get::<_, Option<String>>(6)? {
                            obj.insert("n".into(), serde_json::Value::String(n));
                        }
                        if let Some(f) = r.get::<_, Option<String>>(7)? {
                            obj.insert("f".into(), serde_json::Value::String(f));
                        }
                        if r.get::<_, i64>(8)? != 0 {
                            obj.insert("fail".into(), serde_json::Value::Bool(true));
                        }
                        Ok(serde_json::Value::Object(obj))
                    })
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();

                if !trace.is_empty() {
                    let json = serde_json::to_string(&trace).unwrap();
                    conn.execute("UPDATE work_units SET obs_trace = ?1 WHERE id = ?2", params![json, wu_id]).unwrap();
                }
            }
        }

        // Verify backfilled unit now has obs_trace
        let filled: String = conn
            .query_row(
                "SELECT obs_trace FROM work_units WHERE intent = 'fix auth'",
                [], |r| r.get(0),
            ).unwrap();
        let trace: Vec<serde_json::Value> = serde_json::from_str(&filled).unwrap();
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0]["t"], 1001);
        assert_eq!(trace[1]["t"], 1002);

        // Verify pre-existing obs_trace was NOT overwritten
        let untouched: String = conn
            .query_row(
                "SELECT obs_trace FROM work_units WHERE intent = 'other work'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(untouched, existing_trace, "existing obs_trace must not be overwritten");
    }

    #[test]
    fn friction_flows_into_obs_trace() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");

        // One successful, one failed observation
        insert_obs_classified(
            &conn, "s1", p1, 1001, "file_read", Some("/src/auth.rs"),
            Some("think"), Some("diverge"), None, None, None,
        );
        insert_obs_classified(
            &conn, "s1", p1, 1002, "command", None,
            Some("act"), Some("converge"), None, None,
            Some(r#"{"failed":true}"#),
        );

        // detect_and_store_episodes: annotate (freezes obs_trace) THEN apply_episode_friction
        let count = detect_and_store_episodes(&conn, "s1").unwrap();
        assert_eq!(count, 1);

        // After detect_and_store, friction is applied to observations
        let obs_friction: Vec<Option<String>> = {
            let mut stmt = conn.prepare(
                "SELECT friction FROM observations WHERE session_id = 's1' ORDER BY timestamp"
            ).unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        // The episode has failures > 0, so all obs get "friction" label
        assert_eq!(obs_friction, vec![Some("friction".into()), Some("friction".into())]);

        // But obs_trace was frozen BEFORE apply_episode_friction ran,
        // so friction ("f" key) should be NULL/absent in the trace
        let trace_json: String = conn
            .query_row("SELECT obs_trace FROM work_units WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        let trace: Vec<serde_json::Value> = serde_json::from_str(&trace_json).unwrap();
        for entry in &trace {
            assert!(
                entry.get("f").is_none(),
                "obs_trace is frozen before apply_episode_friction, so friction field must be absent"
            );
        }
    }

    #[test]
    fn obs_trace_count_matches_obs_count() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        let p1 = insert_prompt(&conn, "s1", 1000, "implement the notification system for users");

        // Insert 5 observations
        for i in 0..5 {
            insert_obs_with_prompt(&conn, "s1", p1, 1001 + i, "file_edit", Some("/src/notify.rs"));
        }

        let episodes = detect_episodes(&conn, "s1").unwrap();
        let annotated = annotate_episode(&conn, &episodes[0]).unwrap();

        assert_eq!(annotated.obs_count, 5);
        let trace_json = annotated.obs_trace.expect("obs_trace should be Some");
        let trace: Vec<serde_json::Value> = serde_json::from_str(&trace_json).unwrap();
        assert_eq!(trace.len() as i64, annotated.obs_count, "obs_trace length must equal obs_count");
    }

    #[test]
    fn disabled_summarization_skips_narrative() {
        let conn = setup_db();
        insert_session(&conn, "s1");

        insert_prompt(&conn, "s1", 1000, "fix the authentication bug in the login handler");

        let config = SummarizationConfig::default();
        assert!(!config.enabled);

        let count = detect_and_narrate_episodes(&conn, "s1", &config).unwrap();
        assert_eq!(count, 1);

        // No summary stored since summarization disabled
        let summary: Option<String> = conn
            .query_row(
                "SELECT summary FROM work_units WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(summary.is_none());
    }
}
