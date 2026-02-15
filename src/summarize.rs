use crate::config::SummarizationConfig;
use crate::NmemError;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const SYSTEM_PROMPT: &str =
    "You summarize coding sessions into structured JSON. Return ONLY valid JSON, no markdown, no explanation.";

const USER_PROMPT_TEMPLATE: &str = r#"Summarize this coding session into JSON with these fields:

- "request": One sentence describing the overall goal/task of the session (infer from the pattern of actions)
- "investigated": Files and code that were read to understand the system
- "learned": Key technical facts discovered during the session
- "completed": Concrete actions that were finished successfully
- "next_steps": Logical follow-up work based on what was done
- "files_read": List of file paths that were read
- "files_edited": List of file paths that were modified
- "notes": Any errors encountered, warnings, or important details

Session observations:
{PAYLOAD}

Return ONLY the JSON object."#;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub request: String,
    #[serde(default)]
    pub investigated: Vec<String>,
    #[serde(default)]
    pub learned: Vec<String>,
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub files_read: Vec<String>,
    #[serde(default)]
    pub files_edited: Vec<String>,
    #[serde(default)]
    pub notes: serde_json::Value,
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

    // Gather observations (most recent 50, chronological)
    let mut obs_stmt = conn.prepare(
        "SELECT obs_type, file_path, content FROM observations
         WHERE session_id = ?1
         ORDER BY timestamp ASC LIMIT 50",
    )?;

    out.push_str("Actions:\n");

    let mut rows = obs_stmt.query(params![session_id])?;
    while let Some(row) = rows.next()? {
        let obs_type: String = row.get(0)?;
        let file_path: Option<String> = row.get(1)?;
        let content: String = row.get(2)?;

        let display = if let Some(fp) = &file_path {
            let content_preview: String = content.chars().take(60).collect();
            if content_preview.is_empty() {
                format!("[{obs_type}] {fp}")
            } else {
                format!("[{obs_type}] {fp} - {content_preview}")
            }
        } else {
            let content_preview: String = content.chars().take(80).collect();
            format!("[{obs_type}] {content_preview}")
        };

        out.push_str(&format!("{display}\n"));
    }

    Ok(Some(out))
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_plain_json() {
        let input = r#"{"request": "test"}"#;
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
            "request": "Add auth",
            "investigated": ["src/main.rs"],
            "learned": ["Uses JWT"],
            "completed": ["Added middleware"],
            "next_steps": ["Add tests"],
            "files_read": ["src/main.rs"],
            "files_edited": ["src/auth.rs"],
            "notes": "No errors"
        }"#;
        let summary: SessionSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.request, "Add auth");
        assert_eq!(summary.files_edited, vec!["src/auth.rs"]);
    }

    #[test]
    fn parse_summary_missing_fields() {
        let json = r#"{"request": "Fix bug"}"#;
        let summary: SessionSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.request, "Fix bug");
        assert!(summary.investigated.is_empty());
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
