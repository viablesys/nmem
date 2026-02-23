use crate::s5_config::SummarizationConfig;
use crate::NmemError;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const SYSTEM_PROMPT: &str =
    "You produce structured JSON summaries of coding sessions for an AI agent's cross-session memory. The consumer is the next AI session, not a human. Optimize for context reconstruction. Return ONLY valid JSON, no markdown, no explanation.";

const USER_PROMPT_TEMPLATE: &str = r#"Summarize this coding session for the next AI agent session. The summary will be injected as context so the next session can continue the work without re-deriving conclusions.

Return JSON with these fields:

- "intent": What was being accomplished. Infer from user prompts, agent reasoning, and action patterns — NOT from this instruction. (e.g. "Decouple LLM engine from LM Studio, generalize for S4")
- "learned": Decisions made, trade-offs evaluated, constraints discovered, and conclusions reached. These are things the next session should NOT have to figure out again. Extract from agent reasoning blocks.
- "completed": What was done. Commits, code changes, config changes, tests passed.
- "next_steps": What logically follows. Unfinished work, open questions, known blockers.
- "files_read": File paths that were read
- "files_edited": File paths that were modified
- "notes": Errors encountered, failed approaches, things that didn't work and why

Session data:
{PAYLOAD}

Return ONLY the JSON object."#;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub intent: String,
    #[serde(default, deserialize_with = "string_or_vec")]
    pub learned: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    pub completed: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    pub next_steps: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    pub files_read: Vec<String>,
    #[serde(default, deserialize_with = "string_or_vec")]
    pub files_edited: Vec<String>,
    #[serde(default)]
    pub notes: serde_json::Value,
}

/// Accept either a JSON string or array of strings. Small models sometimes
/// return a single string where we asked for an array.
fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;
    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("string or array of strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_owned()])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(s) = seq.next_element()? {
                out.push(s);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

/// Gather prompts and observations for the session into a text payload.
/// Returns None if fewer than 3 observations exist.
pub fn gather_session_payload(conn: &Connection, session_id: &str) -> Result<Option<String>, NmemError> {
    let obs_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM observations WHERE session_id = ?1",
        params![session_id],
        |r| r.get(0),
    )?;

    if obs_count < 3 {
        return Ok(None);
    }

    let mut out = String::new();

    // Gather user prompts (up to 10, chronological)
    let mut prompt_stmt = conn.prepare(
        "SELECT content FROM prompts
         WHERE session_id = ?1 AND source = 'user'
         ORDER BY timestamp ASC LIMIT 10",
    )?;
    let prompts: Vec<String> = prompt_stmt
        .query_map(params![session_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;

    if !prompts.is_empty() {
        out.push_str("User prompts:\n");
        for p in &prompts {
            let truncated: String = p.chars().take(100).collect();
            out.push_str(&format!("- {truncated}\n"));
        }
        out.push('\n');
    }

    // Gather thinking blocks (up to 5, chronological)
    let mut thinking_stmt = conn.prepare(
        "SELECT content FROM prompts
         WHERE session_id = ?1 AND source = 'agent'
         ORDER BY timestamp ASC LIMIT 5",
    )?;
    let thinking: Vec<String> = thinking_stmt
        .query_map(params![session_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;

    if !thinking.is_empty() {
        out.push_str("Agent reasoning:\n");
        for t in &thinking {
            let truncated: String = t.chars().take(200).collect();
            out.push_str(&format!("- {truncated}\n"));
        }
        out.push('\n');
    }

    // Gather observations (most recent 50, chronological)
    // Include classifier labels and failure metadata for S4 consumers
    let mut obs_stmt = conn.prepare(
        "SELECT obs_type, file_path, content, phase, scope, locus, novelty, metadata
         FROM observations
         WHERE session_id = ?1
         ORDER BY timestamp ASC LIMIT 50",
    )?;

    out.push_str("Actions:\n");

    let mut rows = obs_stmt.query(params![session_id])?;
    while let Some(row) = rows.next()? {
        let obs_type: String = row.get(0)?;
        let file_path: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;
        let phase: Option<String> = row.get(3)?;
        let scope: Option<String> = row.get(4)?;
        let locus: Option<String> = row.get(5)?;
        let novelty: Option<String> = row.get(6)?;
        let metadata_str: Option<String> = row.get(7)?;

        let display = format_action_line(
            &obs_type, file_path.as_deref(), &content,
            phase.as_deref(), scope.as_deref(), locus.as_deref(), novelty.as_deref(),
            metadata_str.as_deref(),
        );
        out.push_str(&format!("{display}\n"));
    }

    Ok(Some(out))
}

/// Format a single observation action line for LLM payloads.
/// Includes classifier stance labels and failure metadata when present.
pub(crate) fn format_action_line(
    obs_type: &str,
    file_path: Option<&str>,
    content: &str,
    phase: Option<&str>,
    scope: Option<&str>,
    locus: Option<&str>,
    novelty: Option<&str>,
    metadata_str: Option<&str>,
) -> String {
    // Build stance tag: [obs_type|think|diverge|novel]
    let mut tag_parts: Vec<&str> = vec![obs_type];
    if let Some(p) = phase { tag_parts.push(p); }
    if let Some(s) = scope { tag_parts.push(s); }
    if let Some(l) = locus { tag_parts.push(l); }
    if let Some(n) = novelty { tag_parts.push(n); }
    let tag = tag_parts.join("|");

    // Base display: [tag] file_path - content_preview
    let base = if let Some(fp) = file_path {
        let preview: String = content.chars().take(60).collect();
        if preview.is_empty() {
            format!("[{tag}] {fp}")
        } else {
            format!("[{tag}] {fp} - {preview}")
        }
    } else {
        let preview: String = content.chars().take(80).collect();
        format!("[{tag}] {preview}")
    };

    // Append failure info if present
    if let Some(ms) = metadata_str {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(ms) {
            if meta.get("failed").and_then(|v| v.as_bool()).unwrap_or(false) {
                let error_preview = meta.get("response")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        let preview: String = s.chars().take(120).collect();
                        preview
                    })
                    .unwrap_or_default();
                if error_preview.is_empty() {
                    return format!("{base} FAILED");
                }
                return format!("{base} FAILED: {error_preview}");
            }
        }
    }

    base
}

/// Strip markdown code fences from LLM response.
fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Skip optional language tag on same line
        let rest = rest.trim_start_matches(|c: char| c != '\n').trim_start_matches('\n');
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
        return rest.trim();
    }
    t
}

/// Call OpenAI-compatible chat completions endpoint, with optional fallback.
fn call_completion(config: &SummarizationConfig, payload: &str) -> Result<SessionSummary, NmemError> {
    let user_content = USER_PROMPT_TEMPLATE.replace("{PAYLOAD}", payload);
    let body = serde_json::json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_content},
        ],
        "temperature": 0.0,
        "max_tokens": 1024,
    });

    let result = try_endpoint(&config.endpoint, &body, config.timeout_secs);

    match result {
        Ok(summary) => Ok(summary),
        Err(primary_err) => {
            if let Some(fallback) = &config.fallback_endpoint {
                eprintln!("nmem: primary endpoint failed ({primary_err}), trying fallback");
                try_endpoint(fallback, &body, config.timeout_secs)
            } else {
                Err(primary_err)
            }
        }
    }
}

fn try_endpoint(
    endpoint: &str,
    body: &serde_json::Value,
    timeout_secs: u64,
) -> Result<SessionSummary, NmemError> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
            .build(),
    );

    let resp: serde_json::Value = agent
        .post(endpoint)
        .send_json(body)
        .map_err(|e| NmemError::Config(format!("summarization request: {e}")))?
        .body_mut()
        .read_json()
        .map_err(|e| NmemError::Config(format!("summarization response: {e}")))?;

    let text = resp
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| NmemError::Config("no content in chat completion response".into()))?;

    let cleaned = strip_fences(text);
    let summary: SessionSummary = serde_json::from_str(cleaned)
        .map_err(|e| NmemError::Config(format!("summary parse: {e}")))?;

    Ok(summary)
}

/// Summarize a session and store the result. Non-fatal — callers should catch errors.
pub fn summarize_session(
    conn: &Connection,
    session_id: &str,
    config: &SummarizationConfig,
) -> Result<(), NmemError> {
    if !config.enabled {
        return Ok(());
    }

    let payload = match gather_session_payload(conn, session_id)? {
        Some(p) => p,
        None => return Ok(()),
    };

    let summary = call_completion(config, &payload)?;
    let summary_json = serde_json::to_string(&summary)?;

    conn.execute(
        "UPDATE sessions SET summary = ?1 WHERE id = ?2",
        params![summary_json, session_id],
    )?;

    // Stream to VictoriaLogs — non-fatal, fire-and-forget
    let project: Option<String> = conn
        .query_row(
            "SELECT project FROM sessions WHERE id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .ok();
    stream_summary_to_logs(session_id, project.as_deref().unwrap_or("unknown"), &summary);

    Ok(())
}

const VLOGS_ENDPOINT: &str = "http://localhost:9428/insert/jsonline";

/// Stream a session summary to VictoriaLogs as a structured log entry.
fn stream_summary_to_logs(session_id: &str, project: &str, summary: &SessionSummary) {
    let completed = summary.completed.join("; ");
    let learned = summary.learned.join("; ");
    let next_steps = summary.next_steps.join("; ");
    let files_edited = summary.files_edited.join(", ");

    let record = serde_json::json!({
        "_msg": summary.intent,
        "service": "nmem",
        "type": "session_summary",
        "session_id": session_id,
        "project": project,
        "intent": summary.intent,
        "learned": learned,
        "completed": completed,
        "next_steps": next_steps,
        "files_edited": files_edited,
    });

    let body = format!("{}\n", record);
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(2)))
            .build(),
    );
    let _ = agent
        .post(VLOGS_ENDPOINT)
        .header("Content-Type", "application/stream+json")
        .send(body.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_plain_json() {
        let input = r#"{"intent": "test"}"#;
        assert_eq!(strip_fences(input), input);
    }

    #[test]
    fn strip_fences_with_json_tag() {
        let input = "```json\n{\"request\": \"test\"}\n```";
        assert_eq!(strip_fences(input), "{\"request\": \"test\"}");
    }

    #[test]
    fn strip_fences_no_tag() {
        let input = "```\n{\"request\": \"test\"}\n```";
        assert_eq!(strip_fences(input), "{\"request\": \"test\"}");
    }

    #[test]
    fn strip_fences_whitespace() {
        let input = "  ```json\n  {\"request\": \"test\"}  \n```  ";
        assert_eq!(strip_fences(input), "{\"request\": \"test\"}");
    }

    #[test]
    fn parse_summary_all_fields() {
        let json = r#"{
            "intent": "Add auth",
            "learned": ["Uses JWT", "Middleware pattern preferred over guard"],
            "completed": ["Added middleware"],
            "next_steps": ["Add tests"],
            "files_read": ["src/main.rs"],
            "files_edited": ["src/auth.rs"],
            "notes": "No errors"
        }"#;
        let summary: SessionSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.intent, "Add auth");
        assert_eq!(summary.files_edited, vec!["src/auth.rs"]);
    }

    #[test]
    fn parse_summary_missing_fields() {
        let json = r#"{"intent": "Fix bug"}"#;
        let summary: SessionSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.intent, "Fix bug");
        assert!(summary.completed.is_empty());
    }

    #[test]
    fn gather_skips_sparse_session() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::schema::MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', 1000)",
            [],
        )
        .unwrap();
        // Only 2 observations — below threshold
        for i in 0..2 {
            conn.execute(
                "INSERT INTO observations (session_id, timestamp, obs_type, source_event, content)
                 VALUES ('s1', ?1, 'file_read', 'PostToolUse', 'content')",
                params![1000 + i],
            )
            .unwrap();
        }

        let result = gather_session_payload(&conn, "s1").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn gather_returns_payload() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::schema::MIGRATIONS.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES ('s1', 'test', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prompts (session_id, timestamp, source, content) VALUES ('s1', 1000, 'user', 'fix the bug')",
            [],
        )
        .unwrap();
        for i in 0..5 {
            conn.execute(
                "INSERT INTO observations (session_id, timestamp, obs_type, source_event, file_path, content)
                 VALUES ('s1', ?1, 'file_read', 'PostToolUse', 'src/main.rs', 'read main')",
                params![1000 + i],
            )
            .unwrap();
        }

        let result = gather_session_payload(&conn, "s1").unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert!(payload.contains("fix the bug"));
        assert!(payload.contains("[file_read]"));
        assert!(payload.contains("src/main.rs"));
    }

    #[test]
    fn disabled_config_returns_ok() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let config = SummarizationConfig::default();
        assert!(!config.enabled);
        let result = summarize_session(&conn, "s1", &config);
        assert!(result.is_ok());
    }
}
