use serde_json::Value;

/// Classify a tool name into an observation type.
pub fn classify_tool(name: &str) -> &'static str {
    match name {
        "Bash" => "command",
        "Read" => "file_read",
        "Write" => "file_write",
        "Edit" => "file_edit",
        "Grep" | "Glob" => "search",
        "Task" => "task_spawn",
        "WebFetch" => "web_fetch",
        "WebSearch" => "web_search",
        _ if name.contains("__") => "mcp_call",
        _ => "tool_other",
    }
}

/// Extract the primary content from a tool invocation.
pub fn extract_content(name: &str, tool_input: &Value) -> String {
    match name {
        "Bash" => tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(500)
            .collect(),
        "Read" | "Write" | "Edit" => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "Grep" => {
            let pattern = tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path = tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path.is_empty() {
                pattern.into()
            } else {
                format!("{pattern} in {path}")
            }
        }
        "Glob" => tool_input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "Task" => tool_input
            .get("description")
            .or_else(|| tool_input.get("prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
        "WebFetch" => tool_input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "WebSearch" => tool_input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        "AskUserQuestion" => tool_input
            .get("questions")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|q| q.get("question"))
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .into(),
        _ => name.into(),
    }
}

/// Extract a file path from tool input, if applicable.
pub fn extract_file_path(name: &str, tool_input: &Value) -> Option<String> {
    match name {
        "Read" | "Write" | "Edit" => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(Into::into),
        "Grep" | "Glob" => tool_input
            .get("path")
            .and_then(|v| v.as_str())
            .map(Into::into),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_classify_tool() {
        assert_eq!(classify_tool("Bash"), "command");
        assert_eq!(classify_tool("Read"), "file_read");
        assert_eq!(classify_tool("Write"), "file_write");
        assert_eq!(classify_tool("Edit"), "file_edit");
        assert_eq!(classify_tool("Grep"), "search");
        assert_eq!(classify_tool("Glob"), "search");
        assert_eq!(classify_tool("Task"), "task_spawn");
        assert_eq!(classify_tool("WebFetch"), "web_fetch");
        assert_eq!(classify_tool("WebSearch"), "web_search");
        assert_eq!(classify_tool("mcp__server__tool"), "mcp_call");
        assert_eq!(classify_tool("Unknown"), "tool_other");
    }

    #[test]
    fn test_extract_content() {
        assert_eq!(
            extract_content("Bash", &json!({"command": "ls -la"})),
            "ls -la"
        );
        assert_eq!(
            extract_content("Read", &json!({"file_path": "/tmp/foo.rs"})),
            "/tmp/foo.rs"
        );
        assert_eq!(
            extract_content("Grep", &json!({"pattern": "TODO", "path": "src/"})),
            "TODO in src/"
        );
        assert_eq!(
            extract_content("Grep", &json!({"pattern": "TODO"})),
            "TODO"
        );
        assert_eq!(
            extract_content("WebSearch", &json!({"query": "rust regex"})),
            "rust regex"
        );
        assert_eq!(extract_content("Unknown", &json!({})), "Unknown");
    }

    #[test]
    fn test_extract_content_truncates_bash() {
        let long_cmd: String = "x".repeat(600);
        let result = extract_content("Bash", &json!({"command": long_cmd}));
        assert_eq!(result.len(), 500);
    }

    #[test]
    fn test_extract_file_path() {
        assert_eq!(
            extract_file_path("Read", &json!({"file_path": "/tmp/f.rs"})),
            Some("/tmp/f.rs".into())
        );
        assert_eq!(
            extract_file_path("Grep", &json!({"path": "src/", "pattern": "x"})),
            Some("src/".into())
        );
        assert_eq!(extract_file_path("Bash", &json!({"command": "ls"})), None);
    }
}
