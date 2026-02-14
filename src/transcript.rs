use crate::NmemError;
use rusqlite::{Connection, params};
use std::io::BufRead;
use std::path::Path;

/// Scan transcript for new thinking blocks, storing them as agent prompts.
/// Returns the prompt_id of the most recent prompt (user or agent).
pub fn scan_transcript(
    conn: &Connection,
    session_id: &str,
    transcript_path: &str,
    ts: i64,
) -> Result<Option<i64>, NmemError> {
    let path = Path::new(transcript_path);
    if !path.exists() {
        return get_current_prompt_id(conn, session_id);
    }

    // Get cursor position
    let cursor: i64 = conn
        .query_row(
            "SELECT line_number FROM _cursor WHERE session_id = ?1",
            params![session_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut latest_prompt_id = get_current_prompt_id(conn, session_id)?;

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);

    let mut new_cursor = cursor;
    for (i, line) in reader.lines().enumerate() {
        let line_num = i as i64;
        if line_num < cursor {
            continue;
        }

        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            new_cursor = line_num + 1;
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                new_cursor = line_num + 1;
                continue;
            }
        };

        if entry.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            new_cursor = line_num + 1;
            continue;
        }

        let content_blocks = entry
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());

        if let Some(blocks) = content_blocks {
            for block in blocks {
                if block.get("type").and_then(|v| v.as_str()) != Some("thinking") {
                    continue;
                }
                let thinking = match block.get("thinking").and_then(|v| v.as_str()) {
                    Some(t) if !t.trim().is_empty() => t.trim(),
                    _ => continue,
                };

                let truncated: String = thinking.chars().take(2000).collect();

                // Dedup: check if we already stored this thinking block
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM prompts WHERE session_id = ?1 AND source = 'agent' AND content = ?2",
                        params![session_id, truncated],
                        |r| r.get(0),
                    )
                    .ok();

                if let Some(id) = existing {
                    latest_prompt_id = Some(id);
                    continue;
                }

                conn.execute(
                    "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?1, ?2, ?3, ?4)",
                    params![session_id, ts, "agent", truncated],
                )?;
                latest_prompt_id = Some(conn.last_insert_rowid());
            }
        }

        new_cursor = line_num + 1;
    }

    // Update cursor
    conn.execute(
        "INSERT OR REPLACE INTO _cursor (session_id, line_number) VALUES (?1, ?2)",
        params![session_id, new_cursor],
    )?;

    Ok(latest_prompt_id)
}

/// Get the most recent prompt_id for a session.
pub fn get_current_prompt_id(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<i64>, NmemError> {
    let id = conn
        .query_row(
            "SELECT id FROM prompts WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            params![session_id],
            |r| r.get(0),
        )
        .ok();
    Ok(id)
}
