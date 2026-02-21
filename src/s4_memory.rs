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
    let (investigate, execute, failures, diverge, converge) = {
        let mut stmt = conn.prepare(
            "SELECT phase, obs_type, scope, COUNT(*) FROM observations
             WHERE session_id = ?1
               AND prompt_id >= ?2 AND prompt_id <= ?3
             GROUP BY phase, obs_type, scope",
        )?;
        let mut inv = 0i64;
        let mut exe = 0i64;
        let mut div = 0i64;
        let mut conv = 0i64;

        let mut rows = stmt.query(params![
            episode.session_id,
            episode.first_prompt_id,
            episode.last_prompt_id,
        ])?;
        while let Some(row) = rows.next()? {
            let phase: Option<String> = row.get(0)?;
            let obs_type: String = row.get(1)?;
            let scope: Option<String> = row.get(2)?;
            let count: i64 = row.get(3)?;
            match phase.as_deref() {
                Some("think") => inv += count,
                Some("act") => exe += count,
                // NULL phase: fall back to obs_type heuristic for old data
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

        (inv, exe, fail, div, conv)
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
    })
    .to_string();

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
    })
}

/// Store annotated episodes in the work_units table.
fn store_episodes(conn: &Connection, episodes: &[WorkUnitRow]) -> Result<(), NmemError> {
    let mut stmt = conn.prepare(
        "INSERT INTO work_units (session_id, started_at, ended_at, intent,
         first_prompt_id, last_prompt_id, hot_files, phase_signature, obs_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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

    // Observations in range
    let mut obs_stmt = conn.prepare(
        "SELECT obs_type, file_path, content FROM observations
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

        let display = if let Some(fp) = &file_path {
            let preview: String = content.chars().take(60).collect();
            if preview.is_empty() {
                format!("[{obs_type}] {fp}")
            } else {
                format!("[{obs_type}] {fp} - {preview}")
            }
        } else {
            let preview: String = content.chars().take(80).collect();
            format!("[{obs_type}] {preview}")
        };
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

/// Orchestrator: detect episodes, annotate, and store. No narrative generation.
pub fn detect_and_store_episodes(
    conn: &Connection,
    session_id: &str,
) -> Result<usize, NmemError> {
    let episodes = detect_episodes(conn, session_id)?;
    if episodes.is_empty() {
        return Ok(0);
    }

    let mut annotated = Vec::with_capacity(episodes.len());
    for ep in &episodes {
        annotated.push(annotate_episode(conn, ep)?);
    }

    store_episodes(conn, &annotated)?;
    Ok(annotated.len())
}

/// Full pipeline: detect, annotate, store, and generate narratives.
pub fn detect_and_narrate_episodes(
    conn: &Connection,
    session_id: &str,
    config: &SummarizationConfig,
) -> Result<usize, NmemError> {
    let episodes = detect_episodes(conn, session_id)?;
    if episodes.is_empty() {
        return Ok(0);
    }

    let mut annotated = Vec::with_capacity(episodes.len());
    for ep in &episodes {
        annotated.push(annotate_episode(conn, ep)?);
    }

    store_episodes(conn, &annotated)?;
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
